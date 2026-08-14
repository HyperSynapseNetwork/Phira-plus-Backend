//! `/api/v1/friends/*` + `/api/v1/users/{phira_id}/block` (contract §1/§16.6, §20).
//!
//! Wires the existing social domain (social/mod.rs) to the HTTP layer that PPF
//! consumes. All responses follow §20: snake_case, `{items,total,page,pageNum}`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::extractors::{ApiJson, ApiPath, ApiQuery};
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
        .route("/friends/{phira_id}/room-invite", post(invite_to_room))
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
    operation_id = "friends_get",
    responses(
        (status = 200, description = "friend list (paginated)", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<PageParams>,
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
    operation_id = "friends_requests_get",
    responses(
        (status = 200, description = "friend requests (paginated)", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn list_requests(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<PageParams>,
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


#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct FriendRequestSendResponse {
    pub id: Uuid,
    pub status: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SendRequestBody {
    pub phira_id: i64,
}

/// POST /api/v1/friends/requests — send a friend request by Phira id.
#[utoipa::path(
    post,
    path = "/api/v1/friends/requests",
    operation_id = "friends_requests_post",
    request_body = SendRequestBody,
    responses(
        (status = 200, description = "request sent", body = FriendRequestSendResponse),
        (status = 409, description = "already friends / request exists", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn send_request(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<SendRequestBody>,
) -> Result<Json<FriendRequestSendResponse>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    state.rate_limiter.check(
        &format!("social-request:{}", auth.sub),
        state.config.rate_limit.social_per_minute,
    )?;
    let db = state.require_db()?;
    let target = crate::users::repo::find_by_phira_id(db, body.phira_id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::UserNotFound, "user not found"))?;
    if crate::social::is_blocked(db, auth.sub, target.id).await? {
        return Err(ApiError::new(ErrorCode::UserBlocked, "user is blocked"));
    }
    let req = crate::social::send_request(db, auth.sub, target.id).await?;
    // The in-app event is committed transactionally with the request. Push is
    // best-effort and can fail independently without rolling back the domain event.
    if let Ok(payload) = crate::social::friend_request_notification_payload(req.id) {
        let _ = state
            .push
            .notify(
                db,
                target.id,
                payload.get("title").and_then(Value::as_str).unwrap_or(""),
                payload.get("body").and_then(Value::as_str).unwrap_or(""),
                Some(&payload),
            )
            .await;
    }
    Ok(Json(FriendRequestSendResponse { id: req.id, status: req.status }))
}

/// POST /api/v1/friends/requests/{id}/accept — accept a friend request.
#[utoipa::path(
    post,
    path = "/api/v1/friends/requests/{request_id}/accept",
    operation_id = "friends_requests_request_id_accept_post",
    responses(
        (status = 204, description = "accepted"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn respond_accept(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(request_id): ApiPath<Uuid>,
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
    operation_id = "friends_requests_request_id_reject_post",
    responses(
        (status = 204, description = "rejected"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn respond_reject(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(request_id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    crate::social::respond_request(db, request_id, auth.sub, false).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoomInviteBody {
    pub room_id: String,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct RoomInviteResponse {
    pub event_id: Uuid,
    pub status: String,
}

/// POST /api/v1/friends/{phira_id}/room-invite — invite an accepted friend.
#[utoipa::path(
    post,
    path = "/api/v1/friends/{phira_id}/room-invite",
    operation_id = "friends_phira_id_room_invite_post",
    request_body = RoomInviteBody,
    responses(
        (status = 200, description = "room invite notification created", body = RoomInviteResponse),
        (status = 403, description = "friend relation required", body = ErrorEnvelope),
        (status = 404, description = "user or room not found", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn invite_to_room(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(phira_id): ApiPath<i64>,
    ApiJson(body): ApiJson<RoomInviteBody>,
) -> Result<Json<RoomInviteResponse>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let room_id = body.room_id.trim();
    if room_id.is_empty() {
        return Err(ApiError::new(ErrorCode::RoomIdRequired, "room_id is required"));
    }
    state.rate_limiter.check(
        &format!("social-invite:{}", auth.sub),
        state.config.rate_limit.social_per_minute,
    )?;
    let db = state.require_db()?;
    let target = crate::users::repo::find_by_phira_id(db, phira_id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::UserNotFound, "user not found"))?;
    if !crate::social::are_friends(db, auth.sub, target.id).await? {
        return Err(ApiError::new(
            ErrorCode::FriendRelationRequired,
            "friend relation required",
        ));
    }
    if crate::social::is_blocked(db, auth.sub, target.id).await? {
        return Err(ApiError::new(ErrorCode::UserBlocked, "user is blocked"));
    }
    state
        .rooms
        .info(room_id)
        .await
        .map_err(|_| ApiError::new(ErrorCode::RoomNotFound, "room not found"))?;

    let action_target = crate::notifications::NotificationActionTarget {
        room_id: Some(room_id.to_string()),
        ..Default::default()
    };
    let actions = crate::notifications::normalize_action_drafts(vec![
        crate::notifications::NotificationActionDraft {
            label: "Join room".to_string(),
            label_key: Some("persistent.actions.joinRoom".to_string()),
            action: crate::notifications::NotificationActionKind::JoinRoom,
            data: action_target.clone(),
            danger: false,
        },
        crate::notifications::NotificationActionDraft {
            label: "Open room".to_string(),
            label_key: Some("persistent.actions.openRoom".to_string()),
            action: crate::notifications::NotificationActionKind::OpenRoom,
            data: action_target,
            danger: false,
        },
    ])?;
    let payload = json!({
        "type": "friend.room_invite",
        "priority": "normal",
        "title": "Room invitation",
        "title_key": "persistent.roomInvite.title",
        "body": format!("A friend invited you to room {room_id}."),
        "body_key": "persistent.roomInvite.body",
        "params": { "room_id": room_id },
        "target": { "room_id": room_id },
        "actions": actions,
        "input": null,
        "deep_link": format!("/room/{room_id}"),
        "dedup_key": format!("friend.room_invite:{}:{}:{}", auth.sub, target.id, room_id),
    });
    let event = crate::notifications::publish_to_users(
        db,
        "friend.room_invite",
        Some(auth.sub),
        payload.clone(),
        &[target.id],
    )
    .await?;
    let _ = state
        .push
        .notify(
            db,
            target.id,
            payload.get("title").and_then(Value::as_str).unwrap_or(""),
            payload.get("body").and_then(Value::as_str).unwrap_or(""),
            Some(&payload),
        )
        .await;
    Ok(Json(RoomInviteResponse {
        event_id: event.id,
        status: "sent".to_string(),
    }))
}

/// POST /api/v1/users/{phira_id}/block — block a user by Phira id.
#[utoipa::path(
    post,
    path = "/api/v1/users/{phira_id}/block",
    operation_id = "users_phira_id_block_post",
    responses(
        (status = 204, description = "blocked"),
        (status = 404, description = "user not found", body = ErrorEnvelope),
    ),
    tag = "friends"
)]
pub async fn block(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(phira_id): ApiPath<i64>,
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

