use axum::{
    extract::{Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use rust_embed::Embed;
use std::sync::Arc;

use crate::{auth, auth::CurrentUser, AppState};

#[derive(Embed)]
#[folder = "frontend/dist/"]
struct FrontendAssets;

pub fn build_router(state: Arc<AppState>) -> Router {
    // API routes that require active user
    let api = Router::new()
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions", post(create_session))
        .route("/api/sessions/{id}", delete(delete_session))
        .route("/api/sessions/{id}", patch(update_session))
        .route("/api/sessions/{id}/status", get(session_status))
        .route("/api/sessions/{id}/logs", get(session_logs))
        .route("/api/sessions/{id}/files", get(list_session_files))
        .route("/api/sessions/{id}/file", get(get_session_file))
        .route("/api/sessions/{id}/file", post(write_session_file))
        .route("/api/sessions/{id}/file", delete(delete_session_file))
        .route("/api/sessions/{id}/file/rename", post(rename_session_file))
        .route("/api/sessions/{id}/upload", post(upload_session_file))
        .route("/api/sessions/{id}/dir", post(create_session_dir))
        .route("/api/sessions/{id}/dir", delete(delete_session_dir))
        .route("/api/sessions/{id}/dir/rename", post(rename_session_dir))
        .route("/api/directories", get(list_directories))
        .route("/api/admin/users", get(crate::admin::list_users))
        .route(
            "/api/admin/users/{id}/approve",
            put(crate::admin::approve_user),
        )
        .route(
            "/api/admin/users/{id}",
            delete(crate::admin::delete_user),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    // /api/me — accessible to both active and pending users (handled in auth middleware)
    let me_api = Router::new()
        .route("/api/me", get(get_me))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::auth_middleware,
        ));

    // OAuth routes (no auth required)
    let auth_routes = Router::new()
        .route("/auth/github", get(crate::oauth::github_redirect))
        .route(
            "/auth/github/callback",
            get(crate::oauth::github_callback),
        )
        .route("/auth/login", post(legacy_login))
        .route("/auth/mode", get(auth_mode));

    let ws = Router::new()
        .route(
            "/ws/term/{session_id}",
            get(crate::ws_handler::ws_terminal),
        )
        .route(
            "/ws/acp/{session_id}",
            get(crate::acp::ws_handler::ws_acp),
        );

    Router::new()
        .merge(api)
        .merge(me_api)
        .merge(auth_routes)
        .merge(ws)
        .route("/assets/{*path}", get(serve_asset))
        .fallback(get(spa_fallback))
        .with_state(state)
}

/// GET /auth/mode — tells frontend which auth mode is available
async fn auth_mode(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let oauth = state.github_client_id.is_some() && state.github_client_secret.is_some();
    Json(serde_json::json!({
        "oauth": oauth,
        "legacy": state.password_hash.is_some(),
    }))
}

/// POST /auth/login — legacy password login, returns token for cookie
async fn legacy_login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let password = body["password"]
        .as_str()
        .ok_or(StatusCode::BAD_REQUEST)?;

    let hash = state.password_hash.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    if !auth::verify_password(password, hash) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    Ok(Json(serde_json::json!({
        "token": password,
        "user": {
            "login": "admin",
            "role": "admin",
            "status": "active",
        }
    })))
}

/// GET /api/me — returns current user info (works for both active and pending)
async fn get_me(
    user: axum::Extension<CurrentUser>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "id": user.id,
        "login": user.login,
        "role": user.role,
        "status": user.status,
        "avatar": user.avatar,
    }))
}

/// Serve static assets from the Vite build output
async fn serve_asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    serve_embedded(&format!("assets/{}", path))
}

/// SPA fallback: serve index.html for any non-API/WS/asset route
async fn spa_fallback(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Try exact file match first (e.g. favicon.svg)
    if !path.is_empty() && !path.contains("..") {
        if let Some(resp) = try_serve_embedded(path) {
            return resp;
        }
    }

    // Fallback to index.html (SPA routing)
    serve_embedded("index.html")
}

fn serve_embedded(path: &str) -> Response {
    try_serve_embedded(path).unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}

fn try_serve_embedded(path: &str) -> Option<Response> {
    FrontendAssets::get(path).map(|file| {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        Response::builder()
            .header("Content-Type", mime.as_ref())
            .header("Cache-Control", "public, max-age=3600")
            .body(axum::body::Body::from(file.data.to_vec()))
            .unwrap()
    })
}

// ── Directory listing ──

#[derive(serde::Deserialize)]
struct DirQuery {
    path: Option<String>,
}

async fn list_directories(
    Query(query): Query<DirQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/ubuntu".to_string());
    let base = query.path.unwrap_or_else(|| home.clone());

    // Security: must be under home directory
    let base_path = std::path::Path::new(&base).canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;
    let home_path = std::path::Path::new(&home).canonicalize()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Home dir error: {}", e)))?;

    if !base_path.starts_with(&home_path) {
        return Err((StatusCode::FORBIDDEN, "Access denied: path must be under home directory".to_string()));
    }

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&base_path)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot read directory: {}", e)))?;

    for entry in read_dir.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !ft.is_dir() { continue; }

        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden dirs and known noisy dirs
        if name.starts_with('.') { continue; }
        if matches!(name.as_str(), "node_modules" | "target" | "__pycache__" | ".git") { continue; }

        let full = entry.path();
        let is_git = full.join(".git").exists();

        entries.push(serde_json::json!({
            "name": name,
            "path": full.to_string_lossy(),
            "is_git": is_git,
        }));
    }

    entries.sort_by(|a, b| {
        a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
    });

    Ok(Json(serde_json::json!({
        "current": base_path.to_string_lossy(),
        "home": home,
        "parent": base_path.parent()
            .filter(|p| p.starts_with(&home_path))
            .map(|p| p.to_string_lossy().to_string()),
        "entries": entries,
    })))
}

// ── Session CRUD ──

#[derive(serde::Deserialize)]
struct CreateSessionReq {
    name: Option<String>,
    #[serde(rename = "type", default = "default_session_type")]
    session_type: crate::session_manager::SessionType,
    work_dir: Option<String>,
}

fn default_session_type() -> crate::session_manager::SessionType {
    crate::session_manager::SessionType::Tmux
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    Json(req): Json<CreateSessionReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let type_label = req.session_type.to_string();
    let work_dir = req.work_dir.unwrap_or_else(|| state.work_dir.clone());

    let name = req.name.unwrap_or_else(|| {
        // Use directory basename as part of session name
        let dir_name = std::path::Path::new(&work_dir)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let count = state.sessions.list_sessions(None).len();
        if dir_name.is_empty() {
            format!("{}-{}", type_label, count + 1)
        } else {
            format!("{}/{}", dir_name, type_label)
        }
    });

    let owner_id = user.id.clone();

    let id = match req.session_type {
        crate::session_manager::SessionType::Tmux => {
            state.sessions
                .create_pty_session(name.clone(), &state.shell, &work_dir, state.default_cols, state.default_rows, &owner_id)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        }
        crate::session_manager::SessionType::Claude => {
            state.sessions
                .create_acp_session(name.clone(), &state.claude_path, &work_dir, state.default_cols, state.default_rows, &owner_id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        }
        crate::session_manager::SessionType::Kiro => {
            state.sessions
                .create_kiro_session(name.clone(), &state.kiro_path, &work_dir, state.default_cols, state.default_rows, &owner_id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        }
    };

    Ok(Json(serde_json::json!({
        "id": id,
        "name": name,
        "type": type_label,
    })))
}

async fn list_sessions(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
) -> Json<serde_json::Value> {
    let filter = if user.is_admin() {
        None // admin sees all
    } else {
        Some(user.id.as_str())
    };
    let sessions = state.sessions.list_sessions(filter);
    Json(serde_json::json!({ "sessions": sessions }))
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> StatusCode {
    // Check ownership (admin can delete any)
    if !user.is_admin() && !state.sessions.is_owner(&id, &user.id) {
        return StatusCode::FORBIDDEN;
    }

    if state.sessions.remove_session(&id) {
        if let Some(ref logger) = state.logger {
            logger.remove_session(&id);
        }
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

async fn session_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let stored_dir = state.sessions.work_dir(&id).ok_or(StatusCode::NOT_FOUND)?;

    // Try to get live cwd from /proc/PID/cwd for PTY sessions
    let live_dir = state.sessions.pty_pid(&id).and_then(|pid| {
        std::fs::read_link(format!("/proc/{}/cwd", pid))
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    });

    let work_dir = live_dir.unwrap_or(stored_dir);
    let dir = std::path::Path::new(&work_dir);

    let git_branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let git_dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout);
            out.lines().count()
        });

    let home = std::env::var("HOME").unwrap_or_default();
    let display_dir = if work_dir.starts_with(&home) {
        work_dir.replacen(&home, "~", 1)
    } else {
        work_dir.clone()
    };

    Ok(Json(serde_json::json!({
        "work_dir": display_dir,
        "git_branch": git_branch,
        "git_dirty": git_dirty.unwrap_or(0),
        "is_git": git_branch.is_some(),
    })))
}

#[derive(serde::Deserialize)]
struct LogsQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn session_logs(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let logger = state.logger.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    let limit = query.limit.unwrap_or(100).min(1000);
    let offset = query.offset.unwrap_or(0);
    let entries = logger.recent_logs(&id, limit, offset);
    Ok(Json(serde_json::json!({
        "entries": entries,
        "count": entries.len(),
    })))
}

// ── Session metadata update ──

#[derive(serde::Deserialize)]
struct UpdateSessionReq {
    description: Option<String>,
    status: Option<crate::session_manager::SessionMeta>,
    notes: Option<String>,
}

async fn update_session(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UpdateSessionReq>,
) -> StatusCode {
    if !user.is_admin() && !state.sessions.is_owner(&id, &user.id) {
        return StatusCode::FORBIDDEN;
    }
    if state.sessions.update_session_meta(&id, req.description, req.status, req.notes) {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── Session file browser ──

#[derive(serde::Deserialize)]
struct FilesQuery {
    pattern: Option<String>,
}

async fn list_session_files(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<FilesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = state.sessions.work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let base = std::path::Path::new(&work_dir).canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid work_dir: {}", e)))?;

    let pattern = query.pattern.as_deref().unwrap_or("*.md");
    let mut files = Vec::new();

    collect_files(&base, &base, pattern, &mut files, 5);

    files.sort_by(|a, b| a["path"].as_str().unwrap_or("").cmp(b["path"].as_str().unwrap_or("")));

    Ok(Json(serde_json::json!({ "files": files })))
}

/// Recursively collect files matching a glob pattern (simple *.ext matching)
fn collect_files(
    dir: &std::path::Path,
    base: &std::path::Path,
    pattern: &str,
    out: &mut Vec<serde_json::Value>,
    max_depth: u32,
) {
    if max_depth == 0 { return; }

    let ext_filter = pattern.strip_prefix("*.");
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden and noisy dirs
        if name.starts_with('.') { continue; }
        if matches!(name.as_str(), "node_modules" | "target" | "__pycache__" | ".git") { continue; }

        if path.is_dir() {
            collect_files(&path, base, pattern, out, max_depth - 1);
        } else if path.is_file() {
            let matches = if let Some(ext) = ext_filter {
                path.extension().map(|e| e == ext).unwrap_or(false)
            } else {
                name == pattern
            };

            if matches {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                let meta = std::fs::metadata(&path);
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = meta.ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                out.push(serde_json::json!({
                    "path": rel.to_string_lossy(),
                    "name": name,
                    "size": size,
                    "modified": modified,
                }));
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct FileQuery {
    path: String,
}

async fn get_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = state.sessions.work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let base = std::path::Path::new(&work_dir).canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid work_dir: {}", e)))?;

    // Security: resolve and check path is under work_dir
    let file_path = base.join(&query.path).canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;

    if !file_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    // Size check (1MB max)
    let meta = std::fs::metadata(&file_path)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("File not found: {}", e)))?;
    if meta.len() > 1_048_576 {
        return Err((StatusCode::BAD_REQUEST, "File too large (max 1MB)".to_string()));
    }

    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot read file: {}", e)))?;

    Ok(Json(serde_json::json!({
        "path": query.path,
        "content": content,
    })))
}

/// Helper: resolve a session work_dir and validate a relative path is under it.
/// Returns (base_canonical, resolved_path). The resolved path may not exist yet (for creates).
fn resolve_session_path(
    state: &AppState,
    session_id: &str,
    rel_path: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), (StatusCode, String)> {
    let work_dir = state.sessions.work_dir(session_id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let base = std::path::Path::new(&work_dir).canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid work_dir: {}", e)))?;

    // For new files, parent must exist and be under base
    let joined = base.join(rel_path);

    // Check for path traversal by normalizing components
    let mut normalized = base.clone();
    for component in std::path::Path::new(rel_path).components() {
        match component {
            std::path::Component::Normal(c) => normalized.push(c),
            std::path::Component::ParentDir => {
                normalized.pop();
                if !normalized.starts_with(&base) {
                    return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
                }
            }
            std::path::Component::CurDir => {}
            _ => return Err((StatusCode::BAD_REQUEST, "Invalid path component".to_string())),
        }
    }

    if !normalized.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    Ok((base, joined))
}

// ── File write (create/edit) ──

#[derive(serde::Deserialize)]
struct WriteFileReq {
    path: String,
    content: String,
}

async fn write_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<WriteFileReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (_base, file_path) = resolve_session_path(&state, &id, &req.path)?;

    // Ensure parent directory exists
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot create dir: {}", e)))?;
    }

    std::fs::write(&file_path, &req.content)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Write failed: {}", e)))?;

    Ok(StatusCode::OK)
}

// ── File delete ──

async fn delete_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &query.path)?;

    let file_path = base.join(&query.path).canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("File not found: {}", e)))?;

    if !file_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !file_path.is_file() {
        return Err((StatusCode::NOT_FOUND, "Not a file".to_string()));
    }

    std::fs::remove_file(&file_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Delete failed: {}", e)))?;

    Ok(StatusCode::OK)
}

// ── File rename ──

#[derive(serde::Deserialize)]
struct RenameReq {
    from: String,
    to: String,
}

async fn rename_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<RenameReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &req.from)?;
    let (_, to_path) = resolve_session_path(&state, &id, &req.to)?;

    let from_path = base.join(&req.from).canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Source not found: {}", e)))?;

    if !from_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if to_path.exists() {
        return Err((StatusCode::CONFLICT, "Destination already exists".to_string()));
    }

    // Ensure parent of destination exists
    if let Some(parent) = to_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot create dir: {}", e)))?;
    }

    std::fs::rename(&from_path, &to_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Rename failed: {}", e)))?;

    Ok(StatusCode::OK)
}

// ── File upload (base64) ──

#[derive(serde::Deserialize)]
struct UploadReq {
    path: String,
    /// Base64-encoded file content
    data: String,
}

async fn upload_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UploadReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (_base, file_path) = resolve_session_path(&state, &id, &req.path)?;

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &req.data)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid base64: {}", e)))?;

    // 10MB limit for uploads
    if bytes.len() > 10_485_760 {
        return Err((StatusCode::BAD_REQUEST, "File too large (max 10MB)".to_string()));
    }

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot create dir: {}", e)))?;
    }

    std::fs::write(&file_path, &bytes)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Write failed: {}", e)))?;

    Ok(StatusCode::OK)
}

// ── Directory operations ──

#[derive(serde::Deserialize)]
struct DirOpReq {
    path: String,
}

async fn create_session_dir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<DirOpReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (_base, dir_path) = resolve_session_path(&state, &id, &req.path)?;

    std::fs::create_dir_all(&dir_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Cannot create dir: {}", e)))?;

    Ok(StatusCode::CREATED)
}

async fn delete_session_dir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<DirOpReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &query.path)?;

    let dir_path = base.join(&query.path).canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Directory not found: {}", e)))?;

    if !dir_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !dir_path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, "Not a directory".to_string()));
    }

    // Don't allow deleting the work_dir root itself
    if dir_path == base {
        return Err((StatusCode::FORBIDDEN, "Cannot delete work directory root".to_string()));
    }

    std::fs::remove_dir_all(&dir_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Delete failed: {}", e)))?;

    Ok(StatusCode::OK)
}

async fn rename_session_dir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<RenameReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &req.from)?;
    let (_, to_path) = resolve_session_path(&state, &id, &req.to)?;

    let from_path = base.join(&req.from).canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Source not found: {}", e)))?;

    if !from_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !from_path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, "Not a directory".to_string()));
    }

    if to_path.exists() {
        return Err((StatusCode::CONFLICT, "Destination already exists".to_string()));
    }

    std::fs::rename(&from_path, &to_path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Rename failed: {}", e)))?;

    Ok(StatusCode::OK)
}
