//! `/api/v1/notifications/*` — PPF Notification Center (contract §8, §20).
//!
//! Wires the existing notification domain (notifications/mod.rs) to the HTTP
//! layer. Action/input re-authenticate every call (§8). All responses are
//! snake_case; inbox is `{items,total,page,pageNum,unread}`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::UserNotificationWithEvent;
use crate::app::AppState;
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

/// Wire shape of one inbox notification (contract §8).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct AppNotificationWire {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub r#type: String,
    pub priority: String,
    pub title: String,
    pub body: Option<Value>,
    pub actor: Value,
    pub target: Value,
    pub actions: Vec<Value>,
    pub input: Option<Value>,
    pub deep_link: String,
    pub expires_at: Option<Value>,
    pub dedup_key: String,
    pub read_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Inbox response (§22 `{items, total, page, pageNum, unread}`).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct NotificationInboxResponse {
    pub items: Vec<AppNotificationWire>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
    pub unread: i64,
}

/// Build the wire shape of one inbox row (contract §8).
async fn wire_notification(
    db: &sqlx::PgPool,
    row: &UserNotificationWithEvent,
) -> Result<AppNotificationWire, ApiError> {
    let payload = &row.payload;
    let actor = if let Some(uid) = row.actor_user_id {
        match crate::users::repo::find_by_id(db, uid).await? {
            Some(u) => json!({ "phira_id": u.phira_id, "username": u.username_cache, "avatar": u.avatar_cache }),
            None => Value::Null,
        }
    } else {
        payload.get("actor").cloned().unwrap_or(Value::Null)
    };
    Ok(AppNotificationWire {
        id: row.id,
        r#type: row.event_type.clone(),
        priority: payload.get("priority").and_then(Value::as_str).unwrap_or("normal").to_string(),
        title: payload.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
        body: payload.get("body").cloned(),
        actor,
        target: payload.get("target").cloned().unwrap_or(Value::Null),
        actions: payload.get("actions").and_then(Value::as_array).cloned().unwrap_or_default(),
        input: payload.get("input").cloned(),
        deep_link: payload.get("deep_link").and_then(Value::as_str).unwrap_or("").to_string(),
        expires_at: payload.get("expires_at").cloned(),
        dedup_key: payload.get("dedup_key").and_then(Value::as_str).unwrap_or("").to_string(),
        read_at: row.read_at,
        created_at: row.created_at,
    })
}

/// GET /api/v1/notifications — inbox (paginated + unread).
#[utoipa::path(
    get,
    path = "/api/v1/notifications",
    operation_id = "notifications_get",
    responses(
        (status = 200, description = "notification inbox", body = NotificationInboxResponse),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<InboxParams>,
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
    for r in &rows {
        items.push(wire_notification(db, r).await?);
    }
    Ok(Json(NotificationInboxResponse {
        items,
        total,
        page,
        page_num,
        unread,
    }))
}

/// POST /api/v1/notifications/{id}/read — mark read.
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/read",
    operation_id = "notifications_id_read_post",
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
    operation_id = "notifications_id_dismiss_post",
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

/// Whitelisted notification action types (§22). Only these may be dispatched:
/// executable actions run backend-side; pure deep-link actions verify the
/// target exists and return (the frontend navigates). Arbitrary Action Registry
/// IDs are rejected.
const EXEC_JOIN_ROOM: &str = "join_room";
const EXEC_FRIEND_ACCEPT: &str = "friend_accept";
const EXEC_FRIEND_REJECT: &str = "friend_reject";
const LINK_ACTIONS: &[&str] = &[
    "open_chart",
    "open_replay",
    "open_room",
    "open_user",
    "open_profile",
];

/// POST /api/v1/notifications/{id}/action — run a notification action.
/// §23 #8: social / navigation actions do NOT require High reauth (session +
/// CSRF + resource policy suffice). The whitelist forbids arbitrary Action
/// Registry IDs, so no elevated context is needed here.
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/action",
    operation_id = "notifications_id_action_post",
    request_body = ActionBody,
    responses(
        (status = 200, description = "action result", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<ActionBody>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let row = super::get_for_user(db, auth.sub, id)
        .await?
        .ok_or_else(|| ApiError::not_found("notification"))?;

    // Resolve the requested button id to its frozen action type (whitelist).
    let actions = row.payload.get("actions").and_then(Value::as_array).cloned().unwrap_or_default();
    let requested = body.action.as_str();
    let mut action_type: Option<String> = None;
    for a in &actions {
        let a_id = a.get("id").and_then(Value::as_str);
        let a_action = a.get("action").and_then(Value::as_str);
        if a_id == Some(requested) || a_action == Some(requested) {
            action_type = a_action.or(a_id).map(str::to_string);
            break;
        }
    }
    let action_type = action_type.ok_or_else(|| ApiError::validation("action not available on this notification"))?;

    let target = row.payload.get("target").cloned().unwrap_or(Value::Null);
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;

    match action_type.as_str() {
        EXEC_JOIN_ROOM => {
            let room_id = target
                .get("room_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::validation("join_room requires target.room_id"))?;
            // §23 #9: presence first. online → force_move → completed;
            // offline → create JoinIntent and stay pending (moves on user.online).
            // Presence uses the player.info `online` field — a player who is
            // online but idling in the lobby (no room_id) still counts as online.
            let online = state
                .player
                .info(user.phira_id as i32)
                .await
                .ok()
                .and_then(|p| p.get("online").and_then(Value::as_bool))
                .unwrap_or(false);
            if online {
                state
                    .rooms
                    .force_move(room_id, user.phira_id as i32, false)
                    .await
                    .map_err(ApiError::from)?;
                Ok(Json(json!({ "status": "completed", "action": action_type, "room_id": room_id })))
            } else {
                let intent = state
                    .join_intents
                    .create(auth.sub, user.phira_id, room_id, None)?;
                Ok(Json(json!({ "status": "pending", "intent_id": intent.id, "action": action_type })))
            }
        }
        EXEC_FRIEND_ACCEPT | EXEC_FRIEND_REJECT => {
            let req_id = target
                .get("friend_request_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| ApiError::validation("friend action requires target.friend_request_id"))?;
            let accept = action_type == EXEC_FRIEND_ACCEPT;
            crate::social::respond_request(db, req_id, auth.sub, accept).await?;
            Ok(Json(json!({ "ok": true, "action": action_type })))
        }
        t if LINK_ACTIONS.contains(&t) => {
            verify_link_target(&state, t, &target).await?;
            Ok(Json(json!({ "ok": true, "action": action_type, "target": target })))
        }
        other => Err(ApiError::validation(format!(
            "notification action type not allowed: {other}"
        ))),
    }
}

/// Best-effort existence check for pure deep-link actions. Network-dependent
/// sources (Phira API / PMP) failing to verify degrade to `ok` (frontend still
/// navigates and shows its own error); a structurally missing target is an error.
async fn verify_link_target(state: &Arc<AppState>, action_type: &str, target: &Value) -> Result<(), ApiError> {
    match action_type {
        "open_room" => {
            let room_id = target
                .get("room_id")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::validation("open_room requires target.room_id"))?;
            let _ = state.rooms.info(room_id).await;
            Ok(())
        }
        "open_chart" => {
            let chart_id = target
                .get("chart_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| ApiError::validation("open_chart requires target.chart_id"))?;
            let _ = state.phira_gateway.chart(chart_id).await;
            Ok(())
        }
        "open_user" | "open_profile" => {
            let phira_id = target
                .get("phira_id")
                .and_then(Value::as_i64)
                .ok_or_else(|| ApiError::validation("open_user requires target.phira_id"))?;
            let _ = state.phira_gateway.user(phira_id).await;
            Ok(())
        }
        // open_replay: existence is verified on the viewer stream itself; a
        // target round_uuid must be present.
        "open_replay" => {
            let _ = target
                .get("round_uuid")
                .and_then(Value::as_str)
                .ok_or_else(|| ApiError::validation("open_replay requires target.round_uuid"))?;
            Ok(())
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct InputBody {
    pub text: String,
}

/// POST /api/v1/notifications/{id}/input — reply (§23 #8: ordinary chat reply
/// does NOT require High reauth; session + CSRF + chat rate-limit suffice).
#[utoipa::path(
    post,
    path = "/api/v1/notifications/{id}/input",
    operation_id = "notifications_id_input_post",
    request_body = InputBody,
    responses(
        (status = 200, description = "input sent", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn input(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<InputBody>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
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
    state
        .rate_limiter
        .check(&format!("chat-send:{room_id}:{}", auth.sub), state.config.rate_limit.chat_send_per_minute)?;
    let result = state
        .rooms
        .chat_send(room_id, user.phira_id as i32, text)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true, "result": result })))
}
