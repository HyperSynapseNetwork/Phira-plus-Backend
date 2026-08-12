//! `/api/v1/notifications/*` — PPF Notification Center (contract §8, §20).
//!
//! Wires the existing notification domain (notifications/mod.rs) to the HTTP
//! layer. Action/input re-authenticate every call (§8). All responses are
//! snake_case; inbox is `{items,total,page,pageNum,unread}`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::UserNotificationWithEvent;
use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/notifications", get(list))
        .route("/notifications/{id}/read", post(read))
        .route("/notifications/{id}/dismiss", post(dismiss))
        .route("/notifications/{id}/action", post(action))
        .route("/notifications/{id}/input", post(input))
}

#[derive(Debug, Deserialize)]
pub struct InboxParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// Build the wire shape of one inbox row (contract §8).
async fn wire_notification(
    db: &sqlx::PgPool,
    row: &UserNotificationWithEvent,
) -> Result<Value, ApiError> {
    let payload = &row.payload;
    let actor = if let Some(uid) = row.actor_user_id {
        match crate::users::repo::find_by_id(db, uid).await? {
            Some(u) => json!({ "phira_id": u.phira_id, "username": u.username_cache, "avatar": u.avatar_cache }),
            None => Value::Null,
        }
    } else {
        payload.get("actor").cloned().unwrap_or(Value::Null)
    };
    Ok(json!({
        "id": row.id,
        "type": row.event_type,
        "priority": payload.get("priority").and_then(Value::as_str).unwrap_or("normal"),
        "title": payload.get("title").and_then(Value::as_str).unwrap_or(""),
        "body": payload.get("body").cloned().unwrap_or(Value::Null),
        "actor": actor,
        "target": payload.get("target").cloned().unwrap_or(Value::Null),
        "actions": payload.get("actions").cloned().unwrap_or(Value::Array(vec![])),
        "input": payload.get("input").cloned().unwrap_or(Value::Null),
        "deep_link": payload.get("deep_link").and_then(Value::as_str).unwrap_or(""),
        "expires_at": payload.get("expires_at").cloned().unwrap_or(Value::Null),
        "dedup_key": payload.get("dedup_key").and_then(Value::as_str).unwrap_or(""),
        "read_at": row.read_at,
        "created_at": row.created_at,
    }))
}

/// GET /api/v1/notifications — inbox (paginated + unread).
#[utoipa::path(
    get,
    path = "/api/v1/notifications",
    responses(
        (status = 200, description = "notification inbox", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<InboxParams>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let page = params.page.unwrap_or(1).max(1);
    let page_num = params.page_num.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * page_num;
    let (rows, total, unread) = super::list_inbox(db, auth.sub, page_num, offset).await?;
    let mut items = Vec::with_capacity(rows.len());
    for r in &rows {
        items.push(wire_notification(db, r).await?);
    }
    Ok(Json(json!({ "items": items, "total": total, "page": page, "pageNum": page_num, "unread": unread })))
}

/// POST /api/v1/notifications/{id}/read — mark read.
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/read",
    responses(
        (status = 204, description = "marked read"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn read(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    super::mark_read(db, id, auth.sub).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// POST /api/v1/notifications/{id}/dismiss — dismiss (hide from inbox).
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/dismiss",
    responses(
        (status = 204, description = "dismissed"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn dismiss(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    super::mark_dismissed(db, id, auth.sub).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ActionBody {
    pub action: String,
}

/// POST /api/v1/notifications/{id}/action — run a notification action (re-auth'd).
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/action",
    request_body = ActionBody,
    responses(
        (status = 200, description = "action acknowledged", body = serde_json::Value),
        (status = 401, description = "reauth required", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<ActionBody>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    // Contract §8: action re-authenticates every execution.
    check_reauth_header(&state, &auth, &headers, ReauthRisk::High)?;
    let db = state.require_db()?;
    let row = super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::not_found("notification"))?;
    let actions = row.payload.get("actions").and_then(Value::as_array).cloned().unwrap_or_default();
    let found = actions.iter().any(|a| {
        a.get("id").and_then(Value::as_str) == Some(body.action.as_str())
            || a.get("action").and_then(Value::as_str) == Some(body.action.as_str())
    });
    if !found {
        return Err(ApiError::validation("action not available on this notification"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InputBody {
    pub text: String,
}

/// POST /api/v1/notifications/{id}/input — reply (contract §8: goes to room.chat_send).
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/input",
    request_body = InputBody,
    responses(
        (status = 200, description = "input sent", body = serde_json::Value),
        (status = 401, description = "reauth required", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn input(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(body): Json<InputBody>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    // Contract §8: input re-authenticates every call.
    check_reauth_header(&state, &auth, &headers, ReauthRisk::High)?;
    let db = state.require_db()?;
    let row = super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::not_found("notification"))?;
    let text = body.text.trim();
    if text.is_empty() || text.chars().count() > 500 {
        return Err(ApiError::validation("input text must be 1..=500 chars"));
    }
    let room_id = row
        .payload
        .get("target")
        .and_then(|t| t.get("room_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::validation("notification is not room-input-able"))?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    let result = state
        .rooms
        .chat_send(room_id, user.phira_id as i32, text)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true, "result": result })))
}
