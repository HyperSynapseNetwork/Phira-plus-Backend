//! `/api/v1/friends/*` + `/api/v1/users/{phira_id}/block` (contract §1/§16.6, §20).
//!
//! Wires the existing social domain (social/mod.rs) to the HTTP layer that PPF
//! consumes. All responses follow §20: snake_case, `{items,total,page,pageNum}`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};
use crate::users::model::User;
// (chrono types flow through `FriendRequest.created_at` directly.)

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/friends", get(list))
        .route("/friends/requests", get(list_requests).post(send_request))
        .route("/friends/requests/{request_id}/accept", post(respond_accept))
        .route("/friends/requests/{request_id}/reject", post(respond_reject))
        .route("/users/{phira_id}/block", post(block))
}

#[derive(Debug, Deserialize)]
pub struct PageParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

fn paginate<T: Clone>(items: Vec<T>, page: i64, page_num: i64) -> (Vec<T>, i64) {
    let total = items.len() as i64;
    let page = page.max(1);
    let page_num = page_num.clamp(1, 100);
    let start = ((page - 1) * page_num).min(total) as usize;
    let end = (start + page_num as usize).min(total as usize);
    (items[start..end].to_vec(), total)
}

fn friend_of(user: &User) -> Value {
    json!({
        "phira_id": user.phira_id,
        "username": user.username_cache,
        "avatar": user.avatar_cache,
        "online_status": Value::Null,
    })
}

async fn user_by_id(db: &sqlx::PgPool, id: Uuid) -> Result<Option<User>, ApiError> {
    crate::users::repo::find_by_id(db, id).await
}

/// GET /api/v1/friends — the caller's friend list (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/friends",
    responses(
        (status = 200, description = "friend list (paginated)", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<PageParams>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let friend_ids = crate::social::list_friends(db, auth.sub).await?;
    let mut friends: Vec<Value> = Vec::with_capacity(friend_ids.len());
    for fid in friend_ids {
        if let Some(u) = user_by_id(db, fid).await? {
            friends.push(friend_of(&u));
        }
    }
    let (items, total) = paginate(friends, params.page.unwrap_or(1), params.page_num.unwrap_or(50));
    Ok(Json(json!({ "items": items, "total": total, "page": params.page.unwrap_or(1).max(1), "pageNum": params.page_num.unwrap_or(50) })))
}

/// GET /api/v1/friends/requests — incoming + outgoing friend requests.
#[utoipa::path(
    get,
    path = "/api/v1/friends/requests",
    responses(
        (status = 200, description = "friend requests (paginated)", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn list_requests(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<PageParams>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let requests = crate::social::list_requests_for_user(db, auth.sub).await?;
    let mut items: Vec<Value> = Vec::with_capacity(requests.len());
    for req in requests {
        let from = match user_by_id(db, req.from_user_id).await? {
            Some(u) => friend_of(&u),
            None => Value::Null,
        };
        let to = match user_by_id(db, req.to_user_id).await? {
            Some(u) => friend_of(&u),
            None => Value::Null,
        };
        items.push(json!({
            "id": req.id,
            "from": from,
            "to": to,
            "status": req.status,
            "created_at": req.created_at,
        }));
    }
    let (slice, total) = paginate(items, params.page.unwrap_or(1), params.page_num.unwrap_or(50));
    Ok(Json(json!({ "items": slice, "total": total, "page": params.page.unwrap_or(1).max(1), "pageNum": params.page_num.unwrap_or(50) })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SendRequestBody {
    pub phira_id: i64,
}

/// POST /api/v1/friends/requests — send a friend request by Phira id.
#[utoipa::path(
    post,
    path = "/api/v1/friends/requests",
    request_body = SendRequestBody,
    responses(
        (status = 200, description = "request sent", body = serde_json::Value),
        (status = 409, description = "already friends / request exists", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn send_request(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SendRequestBody>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let target = crate::users::repo::find_by_phira_id(db, body.phira_id)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    if crate::social::is_blocked(db, auth.sub, target.id).await? {
        return Err(ApiError::new(ErrorCode::Conflict, "blocked"));
    }
    let req = crate::social::send_request(db, auth.sub, target.id).await?;
    Ok(Json(json!({ "id": req.id, "status": req.status })))
}

/// POST /api/v1/friends/requests/{id}/accept — accept a friend request.
#[utoipa::path(
    post,
    path = "/api/v1/friends/requests/{request_id}/accept",
    responses(
        (status = 204, description = "accepted"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn respond_accept(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    crate::social::respond_request(db, request_id, auth.sub, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/v1/friends/requests/{id}/reject — reject a friend request.
#[utoipa::path(
    post,
    path = "/api/v1/friends/requests/{request_id}/reject",
    responses(
        (status = 204, description = "rejected"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn respond_reject(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    crate::social::respond_request(db, request_id, auth.sub, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/v1/users/{phira_id}/block — block a user by Phira id.
#[utoipa::path(
    post,
    path = "/api/v1/users/{phira_id}/block",
    responses(
        (status = 204, description = "blocked"),
        (status = 404, description = "user not found", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn block(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(phira_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let target = crate::users::repo::find_by_phira_id(db, phira_id)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    crate::social::block(db, auth.sub, target.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

