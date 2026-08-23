use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Request, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use std::sync::Arc;

use crate::AppState;

/// Claims for the short-lived proxy token. Deliberately NOT the login JWT:
/// it grants access to one exposed slug only, so a dev server that reads
/// its own cookies can never replay them against the ZeroMux API.
#[derive(serde::Serialize, serde::Deserialize)]
struct ProxyClaims {
    sub: String,  // user id (audit only)
    slug: String, // the one exposure this token unlocks
    exp: usize,
}

const PROXY_TOKEN_TTL_SECS: u64 = 12 * 3600;
const COOKIE_NAME: &str = "zeromux_proxy";

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn mint_proxy_token(state: &AppState, user_id: &str, slug: &str) -> String {
    let claims = ProxyClaims {
        sub: user_id.to_string(),
        slug: slug.to_string(),
        exp: (now_secs() + PROXY_TOKEN_TTL_SECS) as usize,
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.jwt_secret.as_bytes()),
    )
    .unwrap_or_default()
}

fn verify_proxy_token(state: &AppState, token: &str, slug: &str) -> bool {
    let key = DecodingKey::from_secret(state.jwt_secret.as_bytes());
    let mut validation = Validation::default();
    validation.validate_exp = true;
    // ProxyClaims has no `aud`; default validation would reject none anyway.
    validation.set_required_spec_claims(&["exp"]);
    match decode::<ProxyClaims>(token, &key, &validation) {
        Ok(data) => data.claims.slug == slug,
        Err(_) => false,
    }
}

/// Extract the exposure slug from the forwarded hostname. Only hostnames of
/// the exact shape "<hash>-<port>.<base>" match (e.g.
/// "k7f2a9qx-3000.zeromux.awscode.dev" with base "zeromux.awscode.dev").
pub fn slug_from_host(host: &str, base: &str) -> Option<String> {
    let host = host.split(':').next()?; // strip :port if present
    let label = host.strip_suffix(base)?.strip_suffix('.')?;
    // hash-port: 8 base36 chars, dash, digits
    let (hash, port) = label.rsplit_once('-')?;
    if hash.len() != 8
        || !hash.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || port.is_empty()
        || !port.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(label.to_string())
}

/// The hostname the proxy request arrived for. nginx sets X-Forwarded-Host
/// to the original client Host because CloudFront rewrites Host to the origin.
pub fn forwarded_host(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// GET /api/proxy/authorize?slug=<hash>-<port>&redirect=<path>
/// Runs on the MAIN domain under normal auth middleware. Mints a slug-scoped
/// token and bounces the browser to the exposure subdomain, which sets the
/// cookie. Only the exposure owner (or an admin) may authorize.
pub async fn authorize(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<crate::auth::CurrentUser>,
    axum::extract::Query(q): axum::extract::Query<AuthorizeQuery>,
) -> Response {
    let exposure = match state.exposures.lookup(&q.slug) {
        Some(e) => e,
        None => return (StatusCode::NOT_FOUND, "Unknown exposure").into_response(),
    };
    if !user.is_admin() && exposure.owner_id != user.id {
        return (StatusCode::FORBIDDEN, "Not your exposure").into_response();
    }
    let token = mint_proxy_token(&state, &user.id, &exposure.slug);
    let base = proxy_base_domain(&state);
    let redirect = q.redirect.unwrap_or_else(|| "/".to_string());
    // The subdomain's /__zeromux_auth endpoint sets the cookie then redirects.
    let url = format!(
        "https://{}.{}/__zeromux_auth?token={}&redirect={}",
        exposure.slug,
        base,
        token,
        urlencode(&redirect)
    );
    Redirect::temporary(&url).into_response()
}

#[derive(serde::Deserialize)]
pub struct AuthorizeQuery {
    pub slug: String,
    pub redirect: Option<String>,
}

/// POST /api/sessions/{id}/expose {"port": 3000} — create (or fetch) the
/// stable exposure for a port of this session and return its public URL.
pub async fn expose_port(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<crate::auth::CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::Json(body): axum::Json<ExposeReq>,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    if state.sessions.work_dir(&id).is_none() {
        return Err((StatusCode::NOT_FOUND, "Session not found".to_string()));
    }
    if !user.is_admin() && !state.sessions.is_owner(&id, &user.id) {
        return Err((StatusCode::FORBIDDEN, "Forbidden".to_string()));
    }
    if body.port < 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Ports below 1024 are not proxied".to_string(),
        ));
    }
    let exposure = state
        .exposures
        .expose(&id, body.port, &user.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let base = proxy_base_domain(&state);
    Ok(axum::Json(serde_json::json!({
        "slug": exposure.slug,
        "url": format!("https://{}.{}/", exposure.slug, base),
        "shareable": exposure.shareable,
    })))
}

#[derive(serde::Deserialize)]
pub struct ExposeReq {
    pub port: u16,
}

// ── Standalone tunnels: ssh-tunnel-style forwards for services that don't
//    run inside any ZeroMux session. Same proxy/auth path as exposures. ──

/// GET /api/tunnels — list tunnels (with live listen check per port).
pub async fn list_tunnels(
    State(state): State<Arc<AppState>>,
    _user: axum::Extension<crate::auth::CurrentUser>,
) -> axum::Json<serde_json::Value> {
    let base = proxy_base_domain(&state);
    let tunnels: Vec<serde_json::Value> = state
        .exposures
        .list_for_session(crate::exposures::TUNNEL_SESSION)
        .into_iter()
        .map(|e| {
            let listening = std::net::TcpStream::connect_timeout(
                &std::net::SocketAddr::from(([127, 0, 0, 1], e.port)),
                std::time::Duration::from_millis(150),
            )
            .is_ok();
            serde_json::json!({
                "slug": e.slug,
                "name": e.name,
                "port": e.port,
                "url": format!("https://{}.{}/", e.slug, base),
                "listening": listening,
                "created_at": e.created_at,
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "tunnels": tunnels }))
}

#[derive(serde::Deserialize)]
pub struct CreateTunnelReq {
    pub port: u16,
    #[serde(default)]
    pub name: String,
}

/// POST /api/tunnels {"port": 3778, "name": "next-app"}
pub async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<crate::auth::CurrentUser>,
    axum::Json(body): axum::Json<CreateTunnelReq>,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    if body.port < 1024 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Ports below 1024 are not proxied".to_string(),
        ));
    }
    let exposure = state
        .exposures
        .create_tunnel(body.port, body.name.trim(), &user.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let base = proxy_base_domain(&state);
    Ok(axum::Json(serde_json::json!({
        "slug": exposure.slug,
        "name": exposure.name,
        "port": exposure.port,
        "url": format!("https://{}.{}/", exposure.slug, base),
    })))
}

/// DELETE /api/tunnels/{slug}
pub async fn delete_tunnel(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<crate::auth::CurrentUser>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let exposure = state
        .exposures
        .lookup(&slug)
        .ok_or((StatusCode::NOT_FOUND, "Unknown tunnel".to_string()))?;
    if exposure.session_id != crate::exposures::TUNNEL_SESSION {
        return Err((StatusCode::BAD_REQUEST, "Not a tunnel".to_string()));
    }
    if !user.is_admin() && exposure.owner_id != user.id {
        return Err((StatusCode::FORBIDDEN, "Not your tunnel".to_string()));
    }
    state.exposures.remove(&slug);
    Ok(StatusCode::NO_CONTENT)
}

/// Base domain used for port subdomains, derived from external_url
/// (e.g. https://zeromux.awscode.dev → zeromux.awscode.dev).
pub fn proxy_base_domain(state: &AppState) -> String {
    state
        .external_url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// Entry point for every request that arrives with a port-prefixed hostname.
/// Handles the auth-cookie handshake, then streams everything else through
/// to 127.0.0.1:{port}.
pub async fn handle(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
) -> Response {
    let headers = req.headers();
    let host = match forwarded_host(headers) {
        Some(h) => h,
        None => return (StatusCode::BAD_REQUEST, "Missing Host").into_response(),
    };
    let slug = match slug_from_host(&host, &proxy_base_domain(&state)) {
        Some(s) => s,
        None => return (StatusCode::BAD_REQUEST, "Not a proxy hostname").into_response(),
    };
    let exposure = match state.exposures.lookup(&slug) {
        Some(e) => e,
        None => return (StatusCode::NOT_FOUND, "Unknown exposure").into_response(),
    };
    let port = exposure.port;

    // Cookie handshake: /__zeromux_auth?token=...&redirect=...
    if req.uri().path() == "/__zeromux_auth" {
        return auth_handshake(&state, &req, &slug);
    }

    // Shareable exposures skip cookie auth: the unguessable slug is the
    // capability. Private ones require the slug-scoped cookie.
    let authorized = exposure.shareable
        || cookie_value(headers, COOKIE_NAME)
            .map(|t| verify_proxy_token(&state, &t, &slug))
            .unwrap_or(false);
    if !authorized {
        // Browser navigation → bounce via main domain to pick up a token.
        // Non-navigation (fetch/XHR/WS) → plain 401.
        let is_navigation = headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|a| a.contains("text/html"))
            .unwrap_or(false);
        if is_navigation {
            let redirect = req
                .uri()
                .path_and_query()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_else(|| "/".to_string());
            let url = format!(
                "{}/api/proxy/authorize?slug={}&redirect={}",
                state.external_url,
                slug,
                urlencode(&redirect)
            );
            return Redirect::temporary(&url).into_response();
        }
        return (StatusCode::UNAUTHORIZED, "Proxy token missing or expired").into_response();
    }

    forward(req, port).await
}

fn auth_handshake(state: &AppState, req: &Request<Body>, slug: &str) -> Response {
    let query: std::collections::HashMap<String, String> = req
        .uri()
        .query()
        .map(|q| {
            q.split('&')
                .filter_map(|kv| {
                    let (k, v) = kv.split_once('=')?;
                    Some((k.to_string(), urldecode(v)))
                })
                .collect()
        })
        .unwrap_or_default();

    let token = match query.get("token") {
        Some(t) if verify_proxy_token(state, t, slug) => t.clone(),
        _ => return (StatusCode::UNAUTHORIZED, "Invalid proxy token").into_response(),
    };
    let redirect = query
        .get("redirect")
        .cloned()
        .unwrap_or_else(|| "/".to_string());
    // Only allow same-site relative redirects.
    let redirect = if redirect.starts_with('/') && !redirect.starts_with("//") {
        redirect
    } else {
        "/".to_string()
    };

    let cookie = format!(
        "{}={}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age={}",
        COOKIE_NAME, token, PROXY_TOKEN_TTL_SECS
    );
    let mut resp = Redirect::temporary(&redirect).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

/// Stream the request through to 127.0.0.1:{port}, including WebSocket
/// upgrades (hyper 1.x handles Connection: upgrade transparently when we
/// pass the request through and copy the upgrade back).
async fn forward(mut req: Request<Body>, port: u16) -> Response {
    // Origin-form request line ("GET / HTTP/1.1") — many dev servers reject
    // or mis-parse absolute-form, so we connect manually instead of using a
    // pooled client that might pass the absolute URI through.
    let origin_uri: Uri = match req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .parse()
    {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, "Bad target URI").into_response(),
    };

    let is_upgrade = req
        .headers()
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);

    // Rewrite the request for the upstream: local target, Host of the app.
    *req.uri_mut() = origin_uri;
    let host_val = HeaderValue::from_str(&format!("127.0.0.1:{}", port))
        .unwrap_or(HeaderValue::from_static("127.0.0.1"));
    req.headers_mut().insert(header::HOST, host_val);
    // Strip our auth cookie so the upstream app never sees the token.
    strip_cookie(req.headers_mut(), COOKIE_NAME);

    if is_upgrade {
        return forward_upgrade(req, port).await;
    }

    let stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Upstream 127.0.0.1:{} unreachable: {}", port, e),
            )
                .into_response()
        }
    };
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::Builder::new()
        .handshake::<_, Body>(io)
        .await
    {
        Ok(x) => x,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("Handshake failed: {}", e)).into_response()
        }
    };
    tokio::spawn(async move {
        let _ = conn.await;
    });
    match sender.send_request(req).await {
        Ok(resp) => resp.map(Body::new).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("Upstream error: {}", e),
        )
            .into_response(),
    }
}

/// WebSocket / upgrade path: send the request with a raw hyper connection,
/// then splice the two upgraded streams together.
async fn forward_upgrade(req: Request<Body>, port: u16) -> Response {
    let stream = match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Upstream 127.0.0.1:{} unreachable: {}", port, e),
            )
                .into_response()
        }
    };
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(x) => x,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("Handshake failed: {}", e)).into_response()
        }
    };
    // The connection future must be polled with upgrades enabled for the
    // upgrade to complete on the client side.
    tokio::spawn(async move {
        let _ = conn.with_upgrades().await;
    });

    // Keep a handle to the incoming request for its upgrade; forward a clone
    // of head + empty body upstream (upgrade requests carry no body).
    let (parts, body) = req.into_parts();
    let mut upstream_req = Request::builder()
        .method(parts.method.clone())
        .uri(parts.uri.clone());
    for (k, v) in parts.headers.iter() {
        upstream_req = upstream_req.header(k, v);
    }
    let upstream_req = match upstream_req.body(Body::empty()) {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("Bad upgrade request: {}", e))
                .into_response()
        }
    };
    // Reassemble the original request so we can await its upgrade.
    let orig_req = Request::from_parts(parts, body);

    let upstream_resp = match sender.send_request(upstream_req).await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)).into_response()
        }
    };

    if upstream_resp.status() == StatusCode::SWITCHING_PROTOCOLS {
        let (resp_parts, resp_body) = upstream_resp.into_parts();
        let upstream_upgrade = hyper::upgrade::on(Response::from_parts(
            resp_parts.clone(),
            resp_body,
        ));
        tokio::spawn(async move {
            let (client_up, server_up) = match tokio::join!(
                hyper::upgrade::on(orig_req),
                upstream_upgrade
            ) {
                (Ok(c), Ok(s)) => (c, s),
                _ => return,
            };
            let mut client_io = hyper_util::rt::TokioIo::new(client_up);
            let mut server_io = hyper_util::rt::TokioIo::new(server_up);
            let _ = tokio::io::copy_bidirectional(&mut client_io, &mut server_io).await;
        });
        let mut resp = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
        for (k, v) in resp_parts.headers.iter() {
            resp = resp.header(k, v);
        }
        resp.body(Body::empty()).unwrap_or_else(|_| {
            (StatusCode::INTERNAL_SERVER_ERROR, "upgrade response").into_response()
        })
    } else {
        upstream_resp.map(Body::new).into_response()
    }
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in cookie.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(name) {
            if let Some(v) = v.strip_prefix('=') {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn strip_cookie(headers: &mut HeaderMap, name: &str) {
    if let Some(cookie) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) {
        let kept: Vec<&str> = cookie
            .split(';')
            .map(|p| p.trim())
            .filter(|p| !p.starts_with(&format!("{}=", name)))
            .collect();
        if kept.is_empty() {
            headers.remove(header::COOKIE);
        } else if let Ok(v) = HeaderValue::from_str(&kept.join("; ")) {
            headers.insert(header::COOKIE, v);
        }
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// GET /api/sessions/{id}/ports — discover listening TCP ports owned by the
/// session's process tree (so the UI can offer one-click preview links).
pub async fn session_ports(
    State(state): State<Arc<AppState>>,
    user: axum::Extension<crate::auth::CurrentUser>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    if state.sessions.work_dir(&id).is_none() {
        return Err((StatusCode::NOT_FOUND, "Session not found".to_string()));
    }
    if !user.is_admin() && !state.sessions.is_owner(&id, &user.id) {
        return Err((StatusCode::FORBIDDEN, "Forbidden".to_string()));
    }

    // Collect the session's process tree (PTY child + descendants).
    let mut tree: std::collections::HashSet<u32> = std::collections::HashSet::new();
    if let Some(root) = state.sessions.pty_pid(&id) {
        collect_descendants(root, &mut tree);
        // tmux-backed sessions: the pane processes are children of the tmux
        // SERVER, not our attach client. Resolve panes via tmux.
        if let Some(tmux_name) = state.sessions.tmux_session_name(&id) {
            if let Ok(out) = std::process::Command::new("tmux")
                .args(["list-panes", "-s", "-t", &tmux_name, "-F", "#{pane_pid}"])
                .output()
            {
                for line in String::from_utf8_lossy(&out.stdout).lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        collect_descendants(pid, &mut tree);
                    }
                }
            }
        }
    }

    let ports = listening_ports_for(&tree);
    let base = proxy_base_domain(&state);
    // Existing exposures for this session, keyed by port; a port without an
    // exposure has url=null and the UI calls POST .../expose on click.
    let exposures: std::collections::HashMap<u16, crate::exposures::Exposure> = state
        .exposures
        .list_for_session(&id)
        .into_iter()
        .map(|e| (e.port, e))
        .collect();
    let entries: Vec<serde_json::Value> = ports
        .into_iter()
        .map(|p| match exposures.get(&p) {
            Some(e) => serde_json::json!({
                "port": p,
                "slug": e.slug,
                "url": format!("https://{}.{}/", e.slug, base),
                "shareable": e.shareable,
            }),
            None => serde_json::json!({
                "port": p,
                "slug": null,
                "url": null,
                "shareable": false,
            }),
        })
        .collect();
    Ok(axum::Json(serde_json::json!({ "ports": entries })))
}

fn collect_descendants(root: u32, acc: &mut std::collections::HashSet<u32>) {
    if !acc.insert(root) {
        return;
    }
    if let Ok(children) =
        std::fs::read_to_string(format!("/proc/{}/task/{}/children", root, root))
    {
        for c in children.split_whitespace() {
            if let Ok(pid) = c.parse::<u32>() {
                collect_descendants(pid, acc);
            }
        }
    }
}

/// Parse /proc/net/tcp{,6} for LISTEN sockets bound by any pid in `pids`.
fn listening_ports_for(pids: &std::collections::HashSet<u32>) -> Vec<u16> {
    // Map socket inodes owned by our pids.
    let mut inodes: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for pid in pids {
        let fd_dir = format!("/proc/{}/fd", pid);
        if let Ok(entries) = std::fs::read_dir(&fd_dir) {
            for e in entries.flatten() {
                if let Ok(target) = std::fs::read_link(e.path()) {
                    let s = target.to_string_lossy();
                    if let Some(inode) = s
                        .strip_prefix("socket:[")
                        .and_then(|x| x.strip_suffix(']'))
                        .and_then(|x| x.parse::<u64>().ok())
                    {
                        inodes.insert(inode);
                    }
                }
            }
        }
    }

    let mut ports = std::collections::BTreeSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(content) = std::fs::read_to_string(table) {
            for line in content.lines().skip(1) {
                let cols: Vec<&str> = line.split_whitespace().collect();
                // st (col 3) == 0A → LISTEN; local_address col 1; inode col 9
                if cols.len() > 9 && cols[3] == "0A" {
                    if let Some(inode) = cols[9].parse::<u64>().ok() {
                        if inodes.contains(&inode) {
                            if let Some(hex_port) = cols[1].rsplit(':').next() {
                                if let Ok(p) = u16::from_str_radix(hex_port, 16) {
                                    if p >= 1024 {
                                        ports.insert(p);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    ports.into_iter().collect()
}
