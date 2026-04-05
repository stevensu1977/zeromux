use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect, Response},
};
use jsonwebtoken::{encode, EncodingKey, Header};
use std::sync::Arc;

use crate::AppState;

#[derive(serde::Deserialize)]
pub struct CallbackQuery {
    pub code: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct JwtClaims {
    pub sub: String,        // user id
    pub role: String,       // "admin" | "member"
    pub status: String,     // "active" | "pending"
    pub login: String,      // github username
    pub avatar: Option<String>,
    pub exp: usize,
}

/// GET /auth/github — redirect to GitHub authorize URL
pub async fn github_redirect(State(state): State<Arc<AppState>>) -> Response {
    let client_id = match &state.github_client_id {
        Some(id) => id,
        None => {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "OAuth not configured")
                .into_response()
        }
    };

    let callback_url = format!("{}/auth/github/callback", state.external_url);
    let url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read:user",
        client_id,
        urlencoding::encode(&callback_url),
    );
    Redirect::temporary(&url).into_response()
}

/// GET /auth/github/callback — exchange code for token, upsert user, issue JWT
pub async fn github_callback(
    Query(query): Query<CallbackQuery>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let (client_id, client_secret) = match (&state.github_client_id, &state.github_client_secret) {
        (Some(id), Some(secret)) => (id, secret),
        _ => {
            return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "OAuth not configured")
                .into_response()
        }
    };

    // Exchange code for access token
    let token_resp = match exchange_code(client_id, client_secret, &query.code).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("OAuth token exchange failed: {}", e);
            return (axum::http::StatusCode::BAD_GATEWAY, "OAuth token exchange failed")
                .into_response();
        }
    };

    // Fetch GitHub user info
    let gh_user = match fetch_github_user(&token_resp.access_token).await {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("GitHub user fetch failed: {}", e);
            return (axum::http::StatusCode::BAD_GATEWAY, "Failed to fetch GitHub user")
                .into_response();
        }
    };

    // Upsert user in database
    let db = match &state.db {
        Some(db) => db,
        None => {
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Database not initialized")
                .into_response()
        }
    };

    let user = match db.upsert_github_user(
        gh_user.id,
        &gh_user.login,
        gh_user.name.as_deref(),
        gh_user.avatar_url.as_deref(),
        &state.allowed_users,
    ) {
        Ok(u) => u,
        Err(e) => {
            tracing::error!("User upsert failed: {}", e);
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "User creation failed")
                .into_response();
        }
    };

    tracing::info!(
        "OAuth login: {} (role={}, status={})",
        user.github_login,
        user.role,
        user.status
    );

    // Issue JWT
    let jwt = match issue_jwt(&user, &state.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("JWT signing failed: {}", e);
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Token signing failed")
                .into_response();
        }
    };

    // Redirect to frontend with JWT in cookie
    let cookie = format!(
        "zeromux_jwt={}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800",
        jwt
    );

    Response::builder()
        .status(302)
        .header("Location", "/")
        .header("Set-Cookie", cookie)
        .body(axum::body::Body::empty())
        .unwrap()
}

pub fn issue_jwt(user: &crate::db::User, secret: &str) -> Result<String, String> {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as usize
        + 7 * 24 * 3600; // 7 days

    let claims = JwtClaims {
        sub: user.id.clone(),
        role: user.role.clone(),
        status: user.status.clone(),
        login: user.github_login.clone(),
        avatar: user.avatar_url.clone(),
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| format!("JWT encode error: {}", e))
}

// ── GitHub API helpers ──

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(serde::Deserialize)]
struct GitHubUser {
    id: i64,
    login: String,
    name: Option<String>,
    avatar_url: Option<String>,
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
) -> Result<TokenResponse, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post("https://github.com/login/oauth/access_token")
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": client_id,
            "client_secret": client_secret,
            "code": code,
        }))
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| format!("Parse error: {}", e))
}

async fn fetch_github_user(access_token: &str) -> Result<GitHubUser, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "zeromux")
        .send()
        .await
        .map_err(|e| format!("HTTP error: {}", e))?;

    resp.json::<GitHubUser>()
        .await
        .map_err(|e| format!("Parse error: {}", e))
}

// URL encoding helper — minimal implementation to avoid extra dependency
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len() * 3);
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push('%');
                    result.push_str(&format!("{:02X}", b));
                }
            }
        }
        result
    }
}
