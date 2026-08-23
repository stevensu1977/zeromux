use axum::{
    extract::{DefaultBodyLimit, Query, State},
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
        .route("/api/sessions/{id}/tmux", post(tmux_action))
        .route("/api/sessions/{id}/ports", get(crate::proxy::session_ports))
        .route("/api/sessions/{id}/expose", post(crate::proxy::expose_port))
        .route("/api/proxy/authorize", get(crate::proxy::authorize))
        .route("/api/tunnels", get(crate::proxy::list_tunnels))
        .route("/api/tunnels", post(crate::proxy::create_tunnel))
        .route("/api/tunnels/{slug}", patch(crate::proxy::update_tunnel))
        .route("/api/tunnels/{slug}", delete(crate::proxy::delete_tunnel))
        .route("/api/sessions/{id}/logs", get(session_logs))
        .route("/api/sessions/{id}/files", get(list_session_files))
        .route("/api/sessions/{id}/file", get(get_session_file))
        .route("/api/sessions/{id}/file", post(write_session_file))
        .route("/api/sessions/{id}/file", delete(delete_session_file))
        .route("/api/sessions/{id}/file/rename", post(rename_session_file))
        .route("/api/sessions/{id}/upload", post(upload_session_file))
        .route("/api/sessions/{id}/tree", get(list_session_tree))
        .route(
            "/api/sessions/{id}/file/download",
            get(download_session_file),
        )
        .route("/api/sessions/{id}/dir", post(create_session_dir))
        .route("/api/sessions/{id}/dir", delete(delete_session_dir))
        .route("/api/sessions/{id}/dir/rename", post(rename_session_dir))
        .route("/api/sessions/{id}/git/log", get(git_log))
        .route("/api/sessions/{id}/git/show", get(git_show))
        .route("/api/sessions/{id}/notes", get(list_notes))
        .route("/api/sessions/{id}/notes", post(create_note))
        .route("/api/sessions/{id}/notes/{note_id}", delete(delete_note))
        .route("/api/events", get(list_events))
        .route("/api/events/{id}", delete(delete_event))
        .route("/api/context", post(save_to_context))
        .route("/api/sessions/{id}/record/start", post(start_recording))
        .route("/api/sessions/{id}/record/stop", post(stop_recording))
        .route("/api/sessions/{id}/record/status", get(recording_status))
        .route("/api/sessions/{id}/context/files", get(list_context_files))
        .route(
            "/api/sessions/{id}/context/files/delete",
            post(delete_context_files),
        )
        .route("/api/sessions/{id}/context/file", get(read_context_file))
        .route(
            "/api/sessions/{id}/context/file/download",
            get(download_context_file),
        )
        .route("/api/directories", get(list_directories))
        .route("/api/tmux/sessions", get(list_tmux_sessions))
        .route("/api/tmux/sessions/kill", post(kill_tmux_session))
        .route("/api/tmux/sessions/rename", post(rename_tmux_session))
        .route("/api/admin/users", get(crate::admin::list_users))
        .route(
            "/api/admin/users/{id}/approve",
            put(crate::admin::approve_user),
        )
        .route("/api/admin/users/{id}", delete(crate::admin::delete_user))
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
        .route("/auth/github/callback", get(crate::oauth::github_callback))
        .route("/auth/login", post(legacy_login))
        .route("/auth/mode", get(auth_mode));

    // Events POST — uses token query param auth (like WebSocket) for hook access
    let events_ingest = Router::new().route("/api/events", post(create_event));

    let ws = Router::new()
        .route("/ws/term/{session_id}", get(crate::ws_handler::ws_terminal))
        .route("/ws/acp/{session_id}", get(crate::acp::ws_handler::ws_acp));

    let app = Router::new()
        .merge(api)
        .merge(me_api)
        .merge(events_ingest)
        .merge(auth_routes)
        .merge(ws)
        .route("/assets/{*path}", get(serve_asset))
        .fallback(get(spa_fallback))
        .layer(DefaultBodyLimit::max(20 * 1024 * 1024))
        .with_state(state.clone());

    // Host dispatch: requests for "<slug>.<base>" hostnames go to the port
    // proxy; everything else falls through to the normal app router.
    let proxy_state = state.clone();
    Router::new().fallback_service(tower::service_fn(move |req: axum::extract::Request| {
        let app = app.clone();
        let proxy_state = proxy_state.clone();
        async move {
            let base = crate::proxy::proxy_base_domain(&proxy_state);
            let is_proxy_host = crate::proxy::forwarded_host(req.headers())
                .and_then(|h| crate::proxy::slug_from_host(&h, &base))
                .is_some();
            let resp = if is_proxy_host {
                crate::proxy::handle(State(proxy_state), req).await
            } else {
                use tower::ServiceExt;
                match app.oneshot(req).await {
                    Ok(r) => r,
                    Err(never) => match never {},
                }
            };
            Ok::<_, std::convert::Infallible>(resp)
        }
    }))
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
    let password = body["password"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let remember = body["remember"].as_bool().unwrap_or(false);

    let hash = state
        .password_hash
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;

    if !auth::verify_password(password, hash) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let max_age = if remember { 2592000 } else { 604800 };
    Ok(Json(serde_json::json!({
        "token": password,
        "max_age": max_age,
        "user": {
            "login": "admin",
            "role": "admin",
            "status": "active",
        }
    })))
}

/// GET /api/me — returns current user info (works for both active and pending)
async fn get_me(user: axum::Extension<CurrentUser>) -> Json<serde_json::Value> {
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
    let base_path = std::path::Path::new(&base)
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;
    let home_path = std::path::Path::new(&home).canonicalize().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Home dir error: {}", e),
        )
    })?;

    if !base_path.starts_with(&home_path) {
        return Err((
            StatusCode::FORBIDDEN,
            "Access denied: path must be under home directory".to_string(),
        ));
    }

    let mut entries = Vec::new();
    let read_dir = std::fs::read_dir(&base_path).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Cannot read directory: {}", e),
        )
    })?;

    for entry in read_dir.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if !ft.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden dirs and known noisy dirs
        if name.starts_with('.') {
            continue;
        }
        if matches!(
            name.as_str(),
            "node_modules" | "target" | "__pycache__" | ".git"
        ) {
            continue;
        }

        let full = entry.path();
        let is_git = full.join(".git").exists();

        entries.push(serde_json::json!({
            "name": name,
            "path": full.to_string_lossy(),
            "is_git": is_git,
        }));
    }

    entries.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
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

// ── Tmux session listing ──

async fn list_tmux_sessions() -> Json<serde_json::Value> {
    let output = std::process::Command::new("tmux")
        .args([
            "ls",
            "-F",
            "#{session_name}\t#{session_windows}\t#{session_attached}\t#{session_created}",
        ])
        .output();

    let sessions: Vec<serde_json::Value> = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter_map(|line| {
                let fields: Vec<&str> = line.split('\t').collect();
                if fields.len() >= 4 {
                    Some(serde_json::json!({
                        "name": fields[0],
                        "windows": fields[1].parse::<u32>().unwrap_or(0),
                        "attached": fields[2].parse::<u32>().unwrap_or(0),
                        "created": fields[3].parse::<i64>().unwrap_or(0),
                    }))
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    };

    Json(serde_json::json!({ "sessions": sessions }))
}

#[derive(serde::Deserialize)]
struct TmuxActionReq {
    name: String,
    new_name: Option<String>,
}

async fn kill_tmux_session(
    Json(req): Json<TmuxActionReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let output = std::process::Command::new("tmux")
        .args(["kill-session", "-t", &req.name])
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run tmux: {}", e),
            )
        })?;

    if output.status.success() {
        Ok(StatusCode::OK)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err((
            StatusCode::BAD_REQUEST,
            format!("tmux kill-session failed: {}", stderr),
        ))
    }
}

async fn rename_tmux_session(
    Json(req): Json<TmuxActionReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let new_name = req
        .new_name
        .ok_or((StatusCode::BAD_REQUEST, "new_name required".to_string()))?;

    let output = std::process::Command::new("tmux")
        .args(["rename-session", "-t", &req.name, &new_name])
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run tmux: {}", e),
            )
        })?;

    if output.status.success() {
        Ok(StatusCode::OK)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err((
            StatusCode::BAD_REQUEST,
            format!("tmux rename failed: {}", stderr),
        ))
    }
}

// ── Session CRUD ──

#[derive(serde::Deserialize)]
struct CreateSessionReq {
    name: Option<String>,
    #[serde(rename = "type", default = "default_session_type")]
    session_type: crate::session_manager::SessionType,
    work_dir: Option<String>,
    tmux_target: Option<String>,
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

    let name = req
        .name
        .or_else(|| req.tmux_target.clone())
        .unwrap_or_else(|| {
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
        crate::session_manager::SessionType::Tmux => state
            .sessions
            .create_pty_session(
                name.clone(),
                &state.shell,
                &work_dir,
                state.default_cols,
                state.default_rows,
                &owner_id,
                req.tmux_target.as_deref(),
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
        crate::session_manager::SessionType::Claude => state
            .sessions
            .create_acp_session(
                name.clone(),
                &state.claude_path,
                &work_dir,
                state.default_cols,
                state.default_rows,
                &owner_id,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
        crate::session_manager::SessionType::Kiro => state
            .sessions
            .create_kiro_session(
                name.clone(),
                &state.kiro_path,
                &work_dir,
                state.default_cols,
                state.default_rows,
                &owner_id,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
        crate::session_manager::SessionType::Codex => state
            .sessions
            .create_codex_session(
                name.clone(),
                &state.codex_path,
                &work_dir,
                state.default_cols,
                state.default_rows,
                &owner_id,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?,
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

#[derive(serde::Deserialize, Default)]
struct DeleteSessionQuery {
    kill: Option<bool>,
}

async fn delete_session(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<DeleteSessionQuery>,
) -> StatusCode {
    // Check ownership (admin can delete any)
    if !user.is_admin() && !state.sessions.is_owner(&id, &user.id) {
        return StatusCode::FORBIDDEN;
    }

    let removed = if query.kill.unwrap_or(false) {
        state.sessions.kill_session(&id)
    } else {
        state.sessions.remove_session(&id)
    };

    if removed {
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
    if state
        .sessions
        .update_session_meta(&id, req.description, req.status)
    {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ── Notes API ──

#[derive(serde::Deserialize)]
struct CreateNoteReq {
    text: String,
    #[serde(default)]
    tags: Vec<String>,
}

async fn list_notes(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let notes = state
        .notes
        .list_notes(&work_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "notes": notes,
        "work_dir": work_dir,
    })))
}

async fn create_note(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<CreateNoteReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let note = state
        .notes
        .create_note(&work_dir, &req.text, &req.tags, &id, &user.login)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!(note)))
}

async fn delete_note(
    State(state): State<Arc<AppState>>,
    axum::extract::Path((_session_id, note_id)): axum::extract::Path<(String, String)>,
) -> StatusCode {
    match state.notes.delete_note(&note_id) {
        Ok(true) => StatusCode::OK,
        Ok(false) => StatusCode::NOT_FOUND,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ── Session file browser ──

#[derive(serde::Deserialize)]
struct FilesQuery {
    pattern: Option<String>,
    base_dir: Option<String>,
}

async fn list_session_files(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<FilesQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let base = resolve_base_dir(&state, &id, query.base_dir.as_deref())?;

    let pattern = query.pattern.as_deref().unwrap_or("*.md");
    let mut files = Vec::new();

    collect_files(&base, &base, pattern, &mut files, 5);

    files.sort_by(|a, b| {
        a["path"]
            .as_str()
            .unwrap_or("")
            .cmp(b["path"].as_str().unwrap_or(""))
    });

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
    if max_depth == 0 {
        return;
    }

    let ext_filter = pattern.strip_prefix("*.");
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden and noisy dirs
        if name.starts_with('.') {
            continue;
        }
        if matches!(
            name.as_str(),
            "node_modules" | "target" | "__pycache__" | ".git"
        ) {
            continue;
        }

        if path.is_dir() {
            collect_files(&path, base, pattern, out, max_depth - 1);
        } else if path.is_file() {
            let matches = if pattern == "*" {
                true
            } else if pattern.contains(',') {
                let exts: Vec<&str> = pattern
                    .split(',')
                    .filter_map(|p| p.trim().strip_prefix("*."))
                    .collect();
                path.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| exts.iter().any(|ext| e.eq_ignore_ascii_case(ext)))
                    .unwrap_or(false)
            } else if let Some(ext) = ext_filter {
                path.extension().map(|e| e == ext).unwrap_or(false)
            } else {
                name == pattern
            };

            if matches {
                let rel = path.strip_prefix(base).unwrap_or(&path);
                let meta = std::fs::metadata(&path);
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = meta
                    .ok()
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

// ── Tree view (lazy directory listing) ──

#[derive(serde::Deserialize)]
struct TreeQuery {
    path: Option<String>,
    base_dir: Option<String>,
}

async fn list_session_tree(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<TreeQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let base = resolve_base_dir(&state, &id, query.base_dir.as_deref())?;

    let rel_path = query.path.as_deref().unwrap_or(".");
    let dir_path = base
        .join(rel_path)
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;

    if !dir_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !dir_path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, "Not a directory".to_string()));
    }

    let read_dir = std::fs::read_dir(&dir_path).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("Cannot read directory: {}", e),
        )
    })?;

    let mut dirs = Vec::new();
    let mut files = Vec::new();

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files
        if name.starts_with('.') {
            continue;
        }
        // Skip noisy directories
        if matches!(
            name.as_str(),
            "node_modules" | "target" | "__pycache__" | ".git"
        ) {
            continue;
        }

        let entry_path = entry.path();
        let rel = entry_path.strip_prefix(&base).unwrap_or(&entry_path);
        let rel_str = rel.to_string_lossy().to_string();

        if entry_path.is_dir() {
            dirs.push(serde_json::json!({
                "name": name,
                "path": rel_str,
                "type": "dir",
            }));
        } else if entry_path.is_file() {
            let meta = std::fs::metadata(&entry_path);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            let modified = meta
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);

            files.push(serde_json::json!({
                "name": name,
                "path": rel_str,
                "type": "file",
                "size": size,
                "modified": modified,
            }));
        }
    }

    // Sort alphabetically within each group
    dirs.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    files.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });

    // Directories first, then files
    let mut entries = dirs;
    entries.append(&mut files);

    // Compute the relative path for the response
    let response_path = dir_path
        .strip_prefix(&base)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| ".".to_string());
    let response_path = if response_path.is_empty() {
        ".".to_string()
    } else {
        response_path
    };

    Ok(Json(serde_json::json!({
        "path": response_path,
        "entries": entries,
    })))
}

// ── File download (raw binary) ──

#[derive(serde::Deserialize)]
struct DownloadQuery {
    path: String,
    base_dir: Option<String>,
}

async fn download_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<DownloadQuery>,
) -> Result<Response, (StatusCode, String)> {
    let base = resolve_base_dir(&state, &id, query.base_dir.as_deref())?;

    // Security: resolve and check path is under base
    let file_path = base
        .join(&query.path)
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;

    if !file_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !file_path.is_file() {
        return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
    }

    let bytes = std::fs::read(&file_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot read file: {}", e),
        )
    })?;

    let filename = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let response = Response::builder()
        .header("Content-Type", "application/octet-stream")
        .header(
            "Content-Disposition",
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(axum::body::Body::from(bytes))
        .unwrap();

    Ok(response)
}

#[derive(serde::Deserialize)]
struct FileQuery {
    path: String,
    base_dir: Option<String>,
}

async fn get_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let base = resolve_base_dir(&state, &id, query.base_dir.as_deref())?;

    // Security: resolve and check path is under base
    let file_path = base
        .join(&query.path)
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;

    if !file_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    let meta = std::fs::metadata(&file_path)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("File not found: {}", e)))?;

    let is_image = file_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico"
            )
        })
        .unwrap_or(false);

    if is_image {
        // 5MB limit for images
        if meta.len() > 5_242_880 {
            return Err((
                StatusCode::BAD_REQUEST,
                "Image too large (max 5MB)".to_string(),
            ));
        }
        let bytes = std::fs::read(&file_path)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot read file: {}", e)))?;
        let mime = match file_path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "svg" => "image/svg+xml",
            "bmp" => "image/bmp",
            "ico" => "image/x-icon",
            _ => "application/octet-stream",
        };
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
        let data_url = format!("data:{};base64,{}", mime, b64);
        Ok(Json(serde_json::json!({
            "path": query.path,
            "content": data_url,
            "binary": true,
        })))
    } else {
        // 1MB limit for text files
        if meta.len() > 1_048_576 {
            return Err((
                StatusCode::BAD_REQUEST,
                "File too large (max 1MB)".to_string(),
            ));
        }
        match std::fs::read_to_string(&file_path) {
            Ok(content) => Ok(Json(serde_json::json!({
                "path": query.path,
                "content": content,
                "binary": false,
            }))),
            Err(_) => {
                // Binary file that's not an image
                let bytes = std::fs::read(&file_path)
                    .map_err(|e| (StatusCode::BAD_REQUEST, format!("Cannot read file: {}", e)))?;
                let b64 =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
                Ok(Json(serde_json::json!({
                    "path": query.path,
                    "content": b64,
                    "binary": true,
                })))
            }
        }
    }
}

/// Resolve the effective base directory: use base_dir_override if provided, otherwise session work_dir.
/// Security: the resolved path must be under HOME.
fn resolve_base_dir(
    state: &AppState,
    session_id: &str,
    base_dir_override: Option<&str>,
) -> Result<std::path::PathBuf, (StatusCode, String)> {
    let dir = if let Some(bd) = base_dir_override.filter(|s| !s.is_empty()) {
        bd.to_string()
    } else {
        state
            .sessions
            .work_dir(session_id)
            .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?
    };

    let base = std::path::Path::new(&dir)
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/ubuntu".to_string());
    let home_path = std::path::Path::new(&home).canonicalize().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Home dir error: {}", e),
        )
    })?;

    if !base.starts_with(&home_path) {
        return Err((
            StatusCode::FORBIDDEN,
            "Path must be under home directory".to_string(),
        ));
    }

    Ok(base)
}

/// Helper: resolve a session work_dir and validate a relative path is under it.
/// Returns (base_canonical, resolved_path). The resolved path may not exist yet (for creates).
fn resolve_session_path(
    state: &AppState,
    session_id: &str,
    rel_path: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), (StatusCode, String)> {
    let base = resolve_base_dir(state, session_id, None)?;
    resolve_path_with_base(&base, rel_path)
}

/// Validate a relative path against a given base directory.
fn resolve_path_with_base(
    base: &std::path::Path,
    rel_path: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), (StatusCode, String)> {
    let joined = base.join(rel_path);

    // Check for path traversal by normalizing components
    let mut normalized = base.to_path_buf();
    for component in std::path::Path::new(rel_path).components() {
        match component {
            std::path::Component::Normal(c) => normalized.push(c),
            std::path::Component::ParentDir => {
                normalized.pop();
                if !normalized.starts_with(base) {
                    return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
                }
            }
            std::path::Component::CurDir => {}
            _ => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Invalid path component".to_string(),
                ))
            }
        }
    }

    if !normalized.starts_with(base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    Ok((base.to_path_buf(), joined))
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
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot create dir: {}", e),
            )
        })?;
    }

    std::fs::write(&file_path, &req.content).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Write failed: {}", e),
        )
    })?;

    Ok(StatusCode::OK)
}

// ── File delete ──

async fn delete_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<FileQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &query.path)?;

    let file_path = base
        .join(&query.path)
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("File not found: {}", e)))?;

    if !file_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !file_path.is_file() {
        return Err((StatusCode::NOT_FOUND, "Not a file".to_string()));
    }

    std::fs::remove_file(&file_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Delete failed: {}", e),
        )
    })?;

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

    let from_path = base
        .join(&req.from)
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Source not found: {}", e)))?;

    if !from_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if to_path.exists() {
        return Err((
            StatusCode::CONFLICT,
            "Destination already exists".to_string(),
        ));
    }

    // Ensure parent of destination exists
    if let Some(parent) = to_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot create dir: {}", e),
            )
        })?;
    }

    std::fs::rename(&from_path, &to_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Rename failed: {}", e),
        )
    })?;

    Ok(StatusCode::OK)
}

// ── File upload (base64) ──

#[derive(serde::Deserialize)]
struct UploadReq {
    path: String,
    /// Base64-encoded file content
    data: String,
    base_dir: Option<String>,
}

async fn upload_session_file(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<UploadReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let base = resolve_base_dir(&state, &id, req.base_dir.as_deref())?;
    let (_base, file_path) = resolve_path_with_base(&base, &req.path)?;

    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &req.data)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid base64: {}", e)))?;

    // 10MB limit for uploads
    if bytes.len() > 10_485_760 {
        return Err((
            StatusCode::BAD_REQUEST,
            "File too large (max 10MB)".to_string(),
        ));
    }

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot create dir: {}", e),
            )
        })?;
    }

    std::fs::write(&file_path, &bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Write failed: {}", e),
        )
    })?;

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

    std::fs::create_dir_all(&dir_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot create dir: {}", e),
        )
    })?;

    Ok(StatusCode::CREATED)
}

async fn delete_session_dir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<DirOpReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &query.path)?;

    let dir_path = base
        .join(&query.path)
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Directory not found: {}", e)))?;

    if !dir_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !dir_path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, "Not a directory".to_string()));
    }

    // Don't allow deleting the work_dir root itself
    if dir_path == base {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot delete work directory root".to_string(),
        ));
    }

    std::fs::remove_dir_all(&dir_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Delete failed: {}", e),
        )
    })?;

    Ok(StatusCode::OK)
}

async fn rename_session_dir(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<RenameReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let (base, _) = resolve_session_path(&state, &id, &req.from)?;
    let (_, to_path) = resolve_session_path(&state, &id, &req.to)?;

    let from_path = base
        .join(&req.from)
        .canonicalize()
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Source not found: {}", e)))?;

    if !from_path.starts_with(&base) {
        return Err((StatusCode::FORBIDDEN, "Path traversal denied".to_string()));
    }

    if !from_path.is_dir() {
        return Err((StatusCode::BAD_REQUEST, "Not a directory".to_string()));
    }

    if to_path.exists() {
        return Err((
            StatusCode::CONFLICT,
            "Destination already exists".to_string(),
        ));
    }

    std::fs::rename(&from_path, &to_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Rename failed: {}", e),
        )
    })?;

    Ok(StatusCode::OK)
}

// ── Git log / show ──

#[derive(serde::Deserialize)]
struct GitLogQuery {
    limit: Option<usize>,
}

async fn git_log(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<GitLogQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let limit = query.limit.unwrap_or(100).min(500);

    // Use --graph --all to show branch/merge topology.
    // COMMIT_START marker distinguishes commit lines from graph-only lines.
    let marker = "COMMIT_START";
    let sep = "\x01"; // ASCII SOH as field separator — won't appear in commit data
    let format_str = format!("{marker}{sep}%H{sep}%h{sep}%an{sep}%aI{sep}%s{sep}%D");

    let output = std::process::Command::new("git")
        .args([
            "log",
            "--all",
            "--graph",
            &format!("--format={}", format_str),
            &format!("-{}", limit),
        ])
        .current_dir(&work_dir)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git log failed: {}", e),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((
            StatusCode::BAD_REQUEST,
            format!("git log error: {}", stderr),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse lines into entries: each has `graph` (the ASCII art prefix) and optionally `commit`
    let mut entries = Vec::new();
    for line in stdout.lines() {
        if let Some(marker_pos) = line.find(marker) {
            // Commit line: graph chars before marker, commit data after
            let graph = &line[..marker_pos];
            let data = &line[marker_pos + marker.len()..];
            let fields: Vec<&str> = data.split(sep).collect();
            // fields[0] is empty (sep before hash), so fields are: ["", hash, short, author, date, subject, refs]
            if fields.len() >= 6 {
                entries.push(serde_json::json!({
                    "graph": graph,
                    "commit": {
                        "hash": fields[1],
                        "short_hash": fields[2],
                        "author": fields[3],
                        "date": fields[4],
                        "subject": fields[5],
                        "refs": fields.get(6).unwrap_or(&""),
                    }
                }));
            }
        } else {
            // Graph-only line (connector between commits)
            entries.push(serde_json::json!({
                "graph": line,
                "commit": null
            }));
        }
    }

    // Total commit count across all branches
    let total = std::process::Command::new("git")
        .args(["rev-list", "--count", "--all"])
        .current_dir(&work_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<usize>()
                .unwrap_or(0)
        })
        .unwrap_or(0);

    Ok(Json(serde_json::json!({
        "entries": entries,
        "total": total,
    })))
}

#[derive(serde::Deserialize)]
struct GitShowQuery {
    commit: String,
}

async fn git_show(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(query): Query<GitShowQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    // Only allow hex chars to prevent command injection
    if !query.commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err((StatusCode::BAD_REQUEST, "Invalid commit hash".to_string()));
    }

    // Commit metadata
    let sep = "---FIELD---";
    let format_str = format!("%H{sep}%h{sep}%an{sep}%aI{sep}%s{sep}%b");
    let meta_output = std::process::Command::new("git")
        .args([
            "log",
            "-1",
            &format!("--format={}", format_str),
            &query.commit,
        ])
        .current_dir(&work_dir)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git show failed: {}", e),
            )
        })?;

    if !meta_output.status.success() {
        return Err((StatusCode::NOT_FOUND, "Commit not found".to_string()));
    }

    let meta_str = String::from_utf8_lossy(&meta_output.stdout);
    let fields: Vec<&str> = meta_str.split(sep).collect();
    let meta = if fields.len() >= 5 {
        serde_json::json!({
            "hash": fields[0].trim(),
            "short_hash": fields[1].trim(),
            "author": fields[2].trim(),
            "date": fields[3].trim(),
            "subject": fields[4].trim(),
            "body": fields.get(5).unwrap_or(&"").trim(),
        })
    } else {
        serde_json::json!({})
    };

    // Diff content
    let diff_output = std::process::Command::new("git")
        .args(["show", "--format=", "--patch", &query.commit])
        .current_dir(&work_dir)
        .output()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("git show failed: {}", e),
            )
        })?;

    let diff = String::from_utf8_lossy(&diff_output.stdout).to_string();

    // Changed files with line counts
    let files: Vec<serde_json::Value> = std::process::Command::new("git")
        .args(["show", "--format=", "--numstat", &query.commit])
        .current_dir(&work_dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split('\t').collect();
                    if parts.len() >= 3 {
                        Some(serde_json::json!({
                            "additions": parts[0].parse::<i32>().unwrap_or(-1),
                            "deletions": parts[1].parse::<i32>().unwrap_or(-1),
                            "path": parts[2],
                        }))
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(serde_json::json!({
        "commit": meta,
        "diff": diff,
        "files": files,
    })))
}

// ── Agent Events ──

/// POST /api/events — create event (token auth via query param)
async fn create_event(
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::auth::TokenQuery>,
    Json(req): Json<crate::events::CreateEventReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, String)> {
    // Authenticate via token query param (same as WebSocket)
    let authed = query
        .token
        .as_ref()
        .and_then(|t| crate::auth::verify_ws_token(&state, t))
        .is_some();

    if !authed {
        return Err((StatusCode::UNAUTHORIZED, "Unauthorized".to_string()));
    }

    let event = state
        .events
        .create(req)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": event.id,
            "timestamp": event.timestamp,
        })),
    ))
}

/// GET /api/events — list events (requires auth middleware)
async fn list_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<crate::events::EventsQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let events = state
        .events
        .list(&query)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({
        "events": events,
        "total": events.len(),
    })))
}

/// DELETE /api/events/{id} — delete single event
async fn delete_event(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let deleted = state
        .events
        .delete_one(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err((StatusCode::NOT_FOUND, "Event not found".to_string()))
    }
}

// ── Context Save ──

#[derive(serde::Deserialize)]
struct SaveContextReq {
    work_dir: String,
    content: String,
    title: Option<String>,
}

async fn save_to_context(
    Json(req): Json<SaveContextReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let work_dir = std::path::Path::new(&req.work_dir);
    if !work_dir.exists() {
        return Err((
            StatusCode::BAD_REQUEST,
            "work_dir does not exist".to_string(),
        ));
    }

    // Security: must be under HOME
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/ubuntu".to_string());
    let home_path = std::path::Path::new(&home).canonicalize().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Home dir error: {}", e),
        )
    })?;
    let work_canonical = work_dir
        .canonicalize()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid path: {}", e)))?;
    if !work_canonical.starts_with(&home_path) {
        return Err((
            StatusCode::FORBIDDEN,
            "Path must be under home directory".to_string(),
        ));
    }

    // Create .zeromux/context/ directory
    let context_dir = work_canonical.join(".zeromux").join("context");
    std::fs::create_dir_all(&context_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot create context dir: {}", e),
        )
    })?;

    // Today's file
    let today = {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = d.as_secs();
        let days = secs / 86400;
        let mut y = 1970i64;
        let mut remaining = days as i64;
        loop {
            let diy = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                366
            } else {
                365
            };
            if remaining < diy {
                break;
            }
            remaining -= diy;
            y += 1;
        }
        let months: [i64; 12] = if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };
        let mut mo = 1;
        for &md in &months {
            if remaining < md {
                break;
            }
            remaining -= md;
            mo += 1;
        }
        let day = remaining + 1;
        let time_secs = secs % 86400;
        let h = time_secs / 3600;
        let m = (time_secs % 3600) / 60;
        (
            format!("{:04}-{:02}-{:02}", y, mo, day),
            format!("{:02}:{:02}", h, m),
        )
    };

    let file_path = context_dir.join(format!("{}.md", today.0));
    let title = req.title.unwrap_or_else(|| "Context".to_string());

    // Append to today's file
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Cannot open file: {}", e),
            )
        })?;

    write!(
        file,
        "\n## {} ({})\n\n{}\n\n---\n",
        title,
        today.1,
        req.content.trim()
    )
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Write failed: {}", e),
        )
    })?;

    Ok(Json(serde_json::json!({
        "file": file_path.to_string_lossy(),
        "date": today.0,
    })))
}

// ── Recording (tmux pipe-pane) ──

async fn start_recording(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let recording = state
        .sessions
        .start_recording(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let entry = match state.recordings.start(crate::recordings::StartRecording {
        session_id: &id,
        session_name: &recording.session_name,
        tmux_name: &recording.tmux_name,
        work_dir: &recording.work_dir,
        file_path: &recording.file_path,
    }) {
        Ok(entry) => entry,
        Err(e) => {
            let _ = state.sessions.stop_recording(&id);
            return Err((StatusCode::INTERNAL_SERVER_ERROR, e));
        }
    };

    Ok(Json(serde_json::json!({
        "recording": true,
        "file": recording.file_path.to_string_lossy(),
        "record": entry,
    })))
}

async fn stop_recording(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let recording = state
        .sessions
        .stop_recording(&id)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if let Some(ref recording) = recording {
        state
            .recordings
            .finish_by_path(&recording.file_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }

    Ok(Json(serde_json::json!({
        "recording": false,
        "file": recording.as_ref().map(|r| r.file_path.to_string_lossy().to_string()),
        "lines": recording.as_ref().map(|r| r.new_line_count).unwrap_or(0),
    })))
}

async fn recording_status(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let status = state
        .sessions
        .recording_status(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    Ok(Json(serde_json::json!({
        "recording": status.is_some(),
        "file": status.map(|p| p.to_string_lossy().to_string()),
    })))
}

/// GET /api/sessions/{id}/context/files — list context files in .zeromux/context/
async fn list_context_files(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let context_dir = std::path::Path::new(&work_dir)
        .join(".zeromux")
        .join("context");

    let files = state
        .recordings
        .sync_and_list(&work_dir, &context_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "files": files })))
}

/// POST /api/sessions/{id}/context/files/delete — batch delete context files
async fn delete_context_files(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let names = body["names"]
        .as_array()
        .ok_or((StatusCode::BAD_REQUEST, "Missing 'names' array".to_string()))?;

    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let context_dir = std::path::Path::new(&work_dir)
        .join(".zeromux")
        .join("context");
    let mut deleted = 0u32;
    let mut deleted_names = Vec::new();

    for name_val in names {
        let name = name_val.as_str().unwrap_or("");
        if name.is_empty() || name.contains("..") || name.contains('/') || name.contains('\\') {
            continue;
        }
        let file_path = context_dir.join(name);
        if file_path.exists() && file_path.starts_with(&context_dir) {
            if std::fs::remove_file(&file_path).is_ok() {
                deleted += 1;
                deleted_names.push(name.to_string());
            }
        }
    }
    state
        .recordings
        .delete_by_names(&work_dir, &deleted_names)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(serde_json::json!({ "deleted": deleted })))
}

/// GET /api/sessions/{id}/context/file?name=xxx — read context file content
async fn read_context_file(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let name = params
        .get("name")
        .ok_or((StatusCode::BAD_REQUEST, "Missing 'name' param".to_string()))?;

    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err((StatusCode::BAD_REQUEST, "Invalid filename".to_string()));
    }

    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let file_path = std::path::Path::new(&work_dir)
        .join(".zeromux")
        .join("context")
        .join(name);

    if !file_path.exists() {
        return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
    }

    let raw = std::fs::read_to_string(&file_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Read failed: {}", e),
        )
    })?;
    let content = strip_ansi(&raw);

    Ok(Json(serde_json::json!({
        "name": name,
        "content": content,
        "size": content.len(),
    })))
}

/// GET /api/sessions/{id}/context/file/download?name=xxx — download raw context file
async fn download_context_file(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let name = params
        .get("name")
        .ok_or((StatusCode::BAD_REQUEST, "Missing 'name' param".to_string()))?;

    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err((StatusCode::BAD_REQUEST, "Invalid filename".to_string()));
    }

    let work_dir = state
        .sessions
        .work_dir(&id)
        .ok_or((StatusCode::NOT_FOUND, "Session not found".to_string()))?;

    let file_path = std::path::Path::new(&work_dir)
        .join(".zeromux")
        .join("context")
        .join(name);

    if !file_path.exists() {
        return Err((StatusCode::NOT_FOUND, "File not found".to_string()));
    }

    let content = std::fs::read(&file_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Read failed: {}", e),
        )
    })?;

    let disposition = format!("attachment; filename=\"{}\"", name);
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (axum::http::header::CONTENT_DISPOSITION, &disposition),
        ],
        content,
    )
        .into_response())
}

#[derive(serde::Deserialize)]
struct TmuxWindowActionReq {
    action: String,
}

/// Run a whitelisted tmux window/pane action against the session's tmux
/// session. Lets the web UI offer window/split buttons without the client
/// having to emit prefix-key sequences (which depend on the user's tmux
/// config and the state of the pane's foreground program).
async fn tmux_action(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(req): Json<TmuxWindowActionReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    require_session_access(&state, &id, &user)?;

    let tmux_name = state.sessions.tmux_session_name(&id).ok_or((
        StatusCode::BAD_REQUEST,
        "Session has no tmux session".to_string(),
    ))?;

    // Killing the last pane of the last window destroys the whole tmux session
    // (and every agent in it) — refuse instead.
    if req.action == "kill-pane" {
        let counts = std::process::Command::new("tmux")
            .args([
                "display-message",
                "-p",
                "-t",
                &tmux_name,
                "-F",
                "#{window_panes} #{session_windows}",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        if counts.as_deref() == Some("1 1") {
            return Err((
                StatusCode::BAD_REQUEST,
                "Refusing to kill the last pane — it would destroy the tmux session".to_string(),
            ));
        }
    }

    // History scrolling (mobile has no mouse wheel): page-up enters copy-mode
    // and scrolls; scroll-exit returns to the live view.
    if req.action == "page-up" || req.action == "page-down" || req.action == "scroll-exit" {
        if req.action == "page-up" {
            let _ = std::process::Command::new("tmux")
                .args(["copy-mode", "-t", &tmux_name])
                .output();
        }
        let key = match req.action.as_str() {
            "page-up" => "page-up",
            "page-down" => "page-down",
            _ => "cancel",
        };
        let output = std::process::Command::new("tmux")
            .args(["send-keys", "-t", &tmux_name, "-X", key])
            .output()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tmux: {}", e)))?;
        // send-keys -X fails when the pane is not in copy-mode (e.g. scroll-exit
        // after already returning to live view) — treat that as a no-op.
        let _ = output;
        return Ok(Json(serde_json::json!({ "ok": true })));
    }

    let pane_target = format!("{}:.+", tmux_name);
    // Open new windows/splits in the active pane's directory (usually the
    // project dir) rather than the server's cwd. tmux expands the format in -c.
    let args: Vec<&str> = match req.action.as_str() {
        "new-window" => vec!["new-window", "-t", &tmux_name, "-c", "#{pane_current_path}"],
        "next-window" => vec!["next-window", "-t", &tmux_name],
        "prev-window" => vec!["previous-window", "-t", &tmux_name],
        "split-h" => vec!["split-window", "-h", "-t", &tmux_name, "-c", "#{pane_current_path}"],
        "split-v" => vec!["split-window", "-v", "-t", &tmux_name, "-c", "#{pane_current_path}"],
        "next-pane" => vec!["select-pane", "-t", &pane_target],
        "kill-pane" => vec!["kill-pane", "-t", &tmux_name],
        "last-window" => vec!["last-window", "-t", &tmux_name],
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown tmux action: {}", req.action),
            ))
        }
    };

    let output = std::process::Command::new("tmux")
        .args(&args)
        .output()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tmux: {}", e)))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err((StatusCode::BAD_REQUEST, format!("tmux failed: {}", stderr)));
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

fn require_session_access(
    state: &AppState,
    id: &str,
    user: &CurrentUser,
) -> Result<(), (StatusCode, String)> {
    if state.sessions.work_dir(id).is_none() {
        return Err((StatusCode::NOT_FOUND, "Session not found".to_string()));
    }
    if !user.is_admin() && !state.sessions.is_owner(id, &user.id) {
        return Err((StatusCode::FORBIDDEN, "Forbidden".to_string()));
    }
    Ok(())
}

/// Strip ANSI escape sequences from text (CSI, OSC, simple escapes, \r)
fn strip_ansi(input: &str) -> String {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut out = Vec::with_capacity(len);
    let mut i = 0;

    while i < len {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= len {
                break;
            }
            match bytes[i] {
                // CSI: ESC [ ... (params/intermediates) final byte
                b'[' => {
                    i += 1;
                    while i < len && bytes[i] >= 0x20 && bytes[i] <= 0x3f {
                        i += 1; // parameter bytes
                    }
                    while i < len && bytes[i] >= 0x20 && bytes[i] <= 0x2f {
                        i += 1; // intermediate bytes
                    }
                    if i < len {
                        i += 1;
                    } // final byte
                }
                // OSC: ESC ] ... (until BEL or ST)
                b']' => {
                    i += 1;
                    while i < len {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        } // BEL
                        if bytes[i] == 0x1b && i + 1 < len && bytes[i + 1] == b'\\' {
                            i += 2;
                            break; // ST
                        }
                        i += 1;
                    }
                }
                // Two-byte sequences: ESC ( ESC ) ESC = ESC > etc.
                b'(' | b')' | b'*' | b'+' | b'=' | b'>' => {
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            }
        } else if bytes[i] == b'\r' {
            i += 1;
        } else if bytes[i] < 0x20 && bytes[i] != b'\n' && bytes[i] != b'\t' {
            // Strip other control chars (BEL, SI, SO, etc.) but keep newline and tab
            i += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    // Collapse runs of 3+ newlines into 2
    let text = String::from_utf8_lossy(&out);
    let mut result = String::with_capacity(text.len());
    let mut newline_count = 0u32;
    for ch in text.chars() {
        if ch == '\n' {
            newline_count += 1;
            if newline_count <= 2 {
                result.push(ch);
            }
        } else {
            newline_count = 0;
            result.push(ch);
        }
    }
    result
}
