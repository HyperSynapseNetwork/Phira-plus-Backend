//! `/api/v1/notifications/*` — PPF persistent Notification Center.
//!
//! Persistent notifications are not PPNotice. This module owns inbox state,
//! typed buttons, input replies, and verified internal navigation results.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::{
    NotificationActionKind, NotificationActionTarget, NotificationActionWire,
    UserNotificationWithEvent,
};
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::extractors::{ApiJson, ApiPath, ApiQuery};
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

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct InboxParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// Wire shape of one inbox notification. Literal title/body remain as legacy
/// and push fallbacks; first-party system events additionally carry semantic
/// i18n keys + safe params.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct AppNotificationWire {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub r#type: String,
    pub priority: String,
    pub title: String,
    pub title_key: Option<String>,
    pub body: String,
    pub body_key: Option<String>,
    pub params: BTreeMap<String, Value>,
    pub actor: Value,
    pub target: Value,
    pub actions: Vec<NotificationActionWire>,
    pub input: Option<Value>,
    pub deep_link: String,
    pub expires_at: Option<Value>,
    pub dedup_key: String,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct NotificationInboxResponse {
    pub items: Vec<AppNotificationWire>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
    pub unread: i64,
}

async fn wire_notification(
    db: &sqlx::PgPool,
    row: &UserNotificationWithEvent,
) -> Result<AppNotificationWire, ApiError> {
    let payload = &row.payload;
    let actor = if let Some(uid) = row.actor_user_id {
        match crate::users::repo::find_by_id(db, uid).await? {
            Some(u) => json!({
                "phira_id": u.phira_id,
                "username": u.username_cache,
                "avatar": u.avatar_cache,
            }),
            None => Value::Null,
        }
    } else {
        payload.get("actor").cloned().unwrap_or(Value::Null)
    };
    let params = payload
        .get("params")
        .and_then(Value::as_object)
        .map(|object| object.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    Ok(AppNotificationWire {
        id: row.id,
        r#type: row.event_type.clone(),
        priority: payload
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("normal")
            .to_string(),
        title: payload
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        title_key: payload.get("title_key").and_then(Value::as_str).map(str::to_string),
        body: payload
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        body_key: payload.get("body_key").and_then(Value::as_str).map(str::to_string),
        params,
        actor,
        target: payload.get("target").cloned().unwrap_or(Value::Null),
        actions: super::normalize_stored_actions(payload.get("actions"), row.event_id),
        input: payload.get("input").cloned(),
        deep_link: payload
            .get("deep_link")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        expires_at: payload.get("expires_at").cloned(),
        dedup_key: payload
            .get("dedup_key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        read_at: row.read_at,
        created_at: row.created_at,
    })
}

#[utoipa::path(
    get,
    path = "/api/v1/notifications",
    operation_id = "notifications_get",
    params(InboxParams),
    responses(
        (status = 200, description = "notification inbox", body = NotificationInboxResponse),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<InboxParams>,
) -> Result<Json<NotificationInboxResponse>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let page = params.page.unwrap_or(1).max(1);
    let page_num = params.page_num.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * page_num;
    let (rows, total, unread) = super::list_inbox(db, auth.sub, page_num, offset).await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(wire_notification(db, row).await?);
    }
    Ok(Json(NotificationInboxResponse {
        items,
        total,
        page,
        page_num,
        unread,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/read",
    operation_id = "notifications_id_read_post",
    responses(
        (status = 204, description = "marked read"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
        (status = 404, description = "notification not found", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn read(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::NotificationNotFound, "notification not found"))?;
    super::mark_read(db, id, auth.sub).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/dismiss",
    operation_id = "notifications_id_dismiss_post",
    responses(
        (status = 204, description = "dismissed"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
        (status = 404, description = "notification not found", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn dismiss(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::NotificationNotFound, "notification not found"))?;
    super::mark_dismissed(db, id, auth.sub).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ActionBody {
    /// Stable button id from the inbox wire. Action kind cannot be substituted
    /// by the caller.
    pub action: String,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NotificationActionResult {
    Completed { action: NotificationActionKind },
    PendingJoinIntent {
        action: NotificationActionKind,
        intent_id: Uuid,
    },
    Navigate {
        action: NotificationActionKind,
        path: String,
    },
}

#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/action",
    operation_id = "notifications_id_action_post",
    request_body = ActionBody,
    responses(
        (status = 200, description = "typed action result", body = NotificationActionResult),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
        (status = 404, description = "notification not found", body = ErrorEnvelope),
        (status = 422, description = "button not available or target invalid", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ActionBody>,
) -> Result<Json<NotificationActionResult>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let row = super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::NotificationNotFound, "notification not found"))?;

    let requested = super::normalize_stored_actions(row.payload.get("actions"), row.event_id)
        .into_iter()
        .find(|item| item.id == body.action)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotificationActionNotAvailable,
                "action not available on this notification",
            )
        })?;
    let action_type = requested.action;
    let target = requested.data;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::UserNotFound, "user not found"))?;

    match action_type {
        NotificationActionKind::JoinRoom => {
            let room_id = target.room_id.as_deref().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotificationActionTargetInvalid,
                    "join_room requires room_id",
                )
            })?;
            let online = state
                .player
                .info(user.phira_id as i32)
                .await
                .ok()
                .and_then(|value| value.get("online").and_then(Value::as_bool))
                .unwrap_or(false);
            if online {
                state
                    .rooms
                    .force_move(room_id, user.phira_id as i32, false)
                    .await
                    .map_err(ApiError::from)?;
                Ok(Json(NotificationActionResult::Completed { action: action_type }))
            } else {
                let intent = state
                    .join_intents
                    .create(auth.sub, user.phira_id, room_id, None)?;
                Ok(Json(NotificationActionResult::PendingJoinIntent {
                    action: action_type,
                    intent_id: intent.id,
                }))
            }
        }
        NotificationActionKind::FriendAccept | NotificationActionKind::FriendReject => {
            let request_id = target.friend_request_id.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotificationActionTargetInvalid,
                    "friend action requires friend_request_id",
                )
            })?;
            let accept = action_type == NotificationActionKind::FriendAccept;
            crate::social::respond_request(db, request_id, auth.sub, accept).await?;
            Ok(Json(NotificationActionResult::Completed { action: action_type }))
        }
        NotificationActionKind::OpenChart
        | NotificationActionKind::OpenReplay
        | NotificationActionKind::OpenRoom
        | NotificationActionKind::OpenUser
        | NotificationActionKind::OpenProfile => {
            let path = verified_navigation_path(&state, action_type, &target).await?;
            Ok(Json(NotificationActionResult::Navigate {
                action: action_type,
                path,
            }))
        }
    }
}

/// Construct only allowlisted relative PPF paths. No arbitrary URL/deep-link is
/// accepted from a notification button payload.
async fn verified_navigation_path(
    state: &Arc<AppState>,
    action_type: NotificationActionKind,
    target: &NotificationActionTarget,
) -> Result<String, ApiError> {
    fn segment(value: &str) -> String {
        percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
    }

    match action_type {
        NotificationActionKind::OpenRoom => {
            let room_id = target.room_id.as_deref().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotificationActionTargetInvalid,
                    "open_room requires room_id",
                )
            })?;
            let _ = state.rooms.info(room_id).await;
            Ok(format!("/room/{}", segment(room_id)))
        }
        NotificationActionKind::OpenChart => {
            let chart_id = target.chart_id.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotificationActionTargetInvalid,
                    "open_chart requires chart_id",
                )
            })?;
            let _ = state.phira_gateway.chart(chart_id).await;
            Ok(format!("/chart/{chart_id}"))
        }
        NotificationActionKind::OpenUser | NotificationActionKind::OpenProfile => {
            let phira_id = target.phira_id.ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotificationActionTargetInvalid,
                    "open_user requires phira_id",
                )
            })?;
            let _ = state.phira_gateway.user(phira_id).await;
            Ok(format!("/user/{phira_id}"))
        }
        NotificationActionKind::OpenReplay => {
            let round_uuid = target.round_uuid.as_deref().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::NotificationActionTargetInvalid,
                    "open_replay requires round_uuid",
                )
            })?;
            Ok(format!("/replay/{}", segment(round_uuid)))
        }
        NotificationActionKind::JoinRoom
        | NotificationActionKind::FriendAccept
        | NotificationActionKind::FriendReject => Err(ApiError::new(
            ErrorCode::NotificationActionTargetInvalid,
            "action does not navigate",
        )),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InputBody {
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct NotificationInputResponse {
    pub ok: bool,
    /// PMP acknowledgement; nested shape remains owned by PMP.
    pub result: Value,
}

#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/input",
    operation_id = "notifications_id_input_post",
    request_body = InputBody,
    responses(
        (status = 200, description = "input sent", body = NotificationInputResponse),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
        (status = 404, description = "notification not found", body = ErrorEnvelope),
        (status = 422, description = "input invalid or not allowed", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn input(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<InputBody>,
) -> Result<Json<NotificationInputResponse>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let row = super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::NotificationNotFound, "notification not found"))?;
    let text = body.text.trim();
    if text.is_empty() {
        return Err(ApiError::new(
            ErrorCode::NotificationInputEmpty,
            "notification input is empty",
        ));
    }
    if text.chars().count() > 500 {
        return Err(ApiError::new(
            ErrorCode::NotificationInputTooLong,
            "notification input is too long",
        ));
    }
    let room_id = row
        .payload
        .get("target")
        .and_then(|target| target.get("room_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::NotificationInputNotAllowed,
                "notification input is not allowed",
            )
        })?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::UserNotFound, "user not found"))?;
    state.rate_limiter.check(
        &format!("chat-send:{room_id}:{}", auth.sub),
        state.config.rate_limit.chat_send_per_minute,
    )?;
    let result = state
        .rooms
        .chat_send(room_id, user.phira_id as i32, text)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(NotificationInputResponse { ok: true, result }))
}
