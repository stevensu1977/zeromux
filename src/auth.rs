use axum::{
    extract::{Query, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::oauth::JwtClaims;
use crate::AppState;

/// Represents the authenticated user, injected into request extensions.
#[derive(Clone, Debug)]
pub struct CurrentUser {
    pub id: String,
    pub role: String,   // "admin" | "member"
    pub status: String, // "active" | "pending"
    pub login: String,
    pub avatar: Option<String>,
}

impl CurrentUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    /// Synthetic user for legacy password mode
    fn legacy() -> Self {
        Self {
            id: "legacy".to_string(),
            role: "admin".to_string(),
            status: "active".to_string(),
            login: "admin".to_string(),
            avatar: None,
        }
    }
}

pub fn hash_password(password: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    hash_password(password) == hash
}

#[derive(serde::Deserialize, Default)]
pub struct TokenQuery {
    pub token: Option<String>,
}

/// Main auth middleware for API routes.
/// Supports JWT (OAuth mode) and legacy password mode.
/// Injects CurrentUser into request extensions.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TokenQuery>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Try JWT auth first (OAuth mode)
    if let Some(user) = try_jwt_auth(&state, &query, &req) {
        // For non-/api/me routes, require active status
        let path = req.uri().path();
        if !user.is_active() && !path.starts_with("/api/me") {
            return Err(StatusCode::FORBIDDEN);
        }
        req.extensions_mut().insert(user);
        return Ok(next.run(req).await);
    }

    // Fallback: legacy password auth
    if let Some(ref password_hash) = state.password_hash {
        if try_legacy_auth(password_hash, &query, &req) {
            req.extensions_mut().insert(CurrentUser::legacy());
            return Ok(next.run(req).await);
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}

/// Try to extract and verify JWT from cookie, header, or query param.
fn try_jwt_auth(
    state: &AppState,
    query: &TokenQuery,
    req: &Request<axum::body::Body>,
) -> Option<CurrentUser> {
    let secret = &state.jwt_secret;

    // 1. Cookie: zeromux_jwt=...
    if let Some(cookie) = req.headers().get("Cookie") {
        if let Ok(cookie_str) = cookie.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(val) = part.strip_prefix("zeromux_jwt=") {
                    if let Some(user) = decode_jwt(val, secret) {
                        return Some(user);
                    }
                }
            }
        }
    }

    // 2. Authorization: Bearer <jwt>
    if let Some(auth) = req.headers().get("Authorization") {
        if let Ok(auth_str) = auth.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if let Some(user) = decode_jwt(token, secret) {
                    return Some(user);
                }
            }
        }
    }

    // 3. Query param ?token=<jwt>
    if let Some(ref token) = query.token {
        if let Some(user) = decode_jwt(token, secret) {
            return Some(user);
        }
    }

    None
}

fn decode_jwt(token: &str, secret: &str) -> Option<CurrentUser> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    let mut validation = Validation::default();
    validation.validate_exp = true;

    decode::<JwtClaims>(token, &key, &validation)
        .ok()
        .map(|data| CurrentUser {
            id: data.claims.sub,
            role: data.claims.role,
            status: data.claims.status,
            login: data.claims.login,
            avatar: data.claims.avatar,
        })
}

/// Legacy password auth (token mode, no OAuth)
fn try_legacy_auth(
    password_hash: &str,
    query: &TokenQuery,
    req: &Request<axum::body::Body>,
) -> bool {
    // Query param ?token=
    if let Some(ref token) = query.token {
        if verify_password(token, password_hash) {
            return true;
        }
    }

    // Authorization: Bearer <password>
    if let Some(auth) = req.headers().get("Authorization") {
        if let Ok(auth_str) = auth.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                if verify_password(token, password_hash) {
                    return true;
                }
            }
        }
    }

    // Cookie: zeromux_token=
    if let Some(cookie) = req.headers().get("Cookie") {
        if let Ok(cookie_str) = cookie.to_str() {
            for part in cookie_str.split(';') {
                let part = part.trim();
                if let Some(val) = part.strip_prefix("zeromux_token=") {
                    if verify_password(val, password_hash) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Verify a WebSocket token — returns CurrentUser if valid and active.
pub fn verify_ws_token(state: &AppState, token: &str) -> Option<CurrentUser> {
    // Try JWT
    if let Some(user) = decode_jwt(token, &state.jwt_secret) {
        if user.is_active() {
            return Some(user);
        }
        return None; // pending users can't connect WS
    }

    // Fallback: legacy password
    if let Some(ref password_hash) = state.password_hash {
        if verify_password(token, password_hash) {
            return Some(CurrentUser::legacy());
        }
    }

    None
}
