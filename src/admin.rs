use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::Arc;

use crate::auth::CurrentUser;
use crate::AppState;

/// GET /api/admin/users — list all users (admin only)
pub async fn list_users(
    user: axum::Extension<CurrentUser>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !user.is_admin() {
        return Err(StatusCode::FORBIDDEN);
    }

    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let users = db.list_users().map_err(|e| {
        tracing::error!("Failed to list users: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(Json(serde_json::json!({ "users": users })))
}

/// PUT /api/admin/users/{id}/approve — approve a pending user
pub async fn approve_user(
    user: axum::Extension<CurrentUser>,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    if !user.is_admin() {
        return Err(StatusCode::FORBIDDEN);
    }

    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let approved = db.approve_user(&user_id).map_err(|e| {
        tracing::error!("Failed to approve user: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if approved {
        tracing::info!("Admin {} approved user {}", user.login, user_id);
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

/// DELETE /api/admin/users/{id} — remove a user
pub async fn delete_user(
    user: axum::Extension<CurrentUser>,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    if !user.is_admin() {
        return Err(StatusCode::FORBIDDEN);
    }

    // Prevent self-deletion
    if user.id == user_id {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = state.db.as_ref().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let deleted = db.delete_user(&user_id).map_err(|e| {
        tracing::error!("Failed to delete user: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if deleted {
        tracing::info!("Admin {} deleted user {}", user.login, user_id);
        Ok(StatusCode::OK)
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
