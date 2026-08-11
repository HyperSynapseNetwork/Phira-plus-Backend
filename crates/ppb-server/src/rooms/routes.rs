//! Room REST routes: `/api/v1/rooms/*` (public + host) and `/api/v1/admin/rooms/*`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::actions::types::Risk;
use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
use crate::commands::broker::{redact_args, CommandTask};
use crate::commands::repo as command_repo;
use crate::error::{ApiError, ErrorCode};

/// Public + logged-in room routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rooms", get(list_rooms))
        .route("/rooms/{room_id}", get(room_info))
        .route("/rooms/{room_id}/history", get(room_history))
        .route("/rooms/{room_id}/chat-history", get(room_chat_history))
        .route("/rooms/{room_id}/chat", post(send_chat))
        .route("/rooms/{room_id}/actions/{action}", post(room_action))
}

/// Admin room routes (permission-gated superset).
pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rooms", get(admin_list_rooms).post(admin_create_room))
        .route("/rooms/{room_id}", get(admin_room_info).delete(admin_close_room))
        .route("/rooms/{room_id}/banlist", get(room_banlist))
        .route("/rooms/{room_id}/whitelist", get(room_whitelist))
}

// ── Public / host routes ───────────────────────────────────────

async fn list_rooms(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let result = state.rooms.list().await.map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn room_info(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let result = state.rooms.info(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn room_history(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let result = state.rooms.history(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn room_chat_history(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let result = state.rooms.chat_history(&room_id, None).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct ChatSendBody {
    pub content: String,
}

/// POST /api/v1/rooms/{room_id}/chat — send a room chat message as the caller.
/// Client must not specify a trusted user_id (design §13.3).
async fn send_chat(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
    Json(body): Json<ChatSendBody>,
) -> Result<Json<Value>, ApiError> {
    let content = body.content.trim();
    if content.is_empty() || content.chars().count() > 500 {
        return Err(ApiError::validation("chat content must be 1..=500 chars"));
    }
    state
        .rate_limiter
        .check(&format!("chat-send:{room_id}"), state.config.rate_limit.chat_send_per_minute)?;
    let phira_id = caller_phira_id(&state, &auth).await?;
    let result = state
        .rooms
        .chat_send(&room_id, phira_id, content)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RoomActionBody {
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/rooms/{room_id}/actions/{action} — host-or-permission action.
async fn room_action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((room_id, action_id)): Path<(String, String)>,
    Json(body): Json<RoomActionBody>,
) -> Result<axum::response::Response, ApiError> {
    let action = state
        .actions
        .get(&action_id)
        .ok_or_else(|| ApiError::not_found("action"))?;

    // Merge room_id into args (host/resource checks need it).
    let mut args = body.args;
    if args.get("room_id").is_none() {
        if let Value::Object(map) = &mut args {
            map.insert("room_id".to_string(), json!(room_id));
        }
    }

    // Authorization: admin permission OR (host_allowed AND real host).
    let has_permission = state
        .permissions
        .has_permission(&state.db, &auth, action.permission)
        .await?;
    let is_host = if action.host_allowed {
        verify_real_host(&state, &auth, &args).await?
    } else {
        false
    };
    if !has_permission && !is_host {
        return Err(ApiError::permission_denied());
    }

    if action.reauth {
        let risk = if action.risk >= Risk::Critical {
            ReauthRisk::Critical
        } else {
            ReauthRisk::High
        };
        check_reauth_header(&state, &auth, &headers, risk)?;
    }

    let db = state.require_db()?;
    let queue_key = state.actions.resolve_queue_key(action, &args);
    let command_id = Uuid::new_v4();
    let args_redacted = redact_args(&args);
    command_repo::insert_queued(db, command_id, action.id, &auth.sub.to_string(), &queue_key, args_redacted.clone())
        .await?;

    let (completion, rx) = if action.long_running {
        (None, None)
    } else {
        let (tx, rx) = oneshot::channel();
        (Some(tx), Some(rx))
    };

    state
        .commands
        .submit(CommandTask {
            command_id,
            action: action.id.to_string(),
            actor: auth.sub.to_string(),
            resource_key: queue_key,
            args,
            args_redacted,
            completion,
        })?;

    if let Some(rx) = rx {
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(v))) => Ok(Json(v).into_response()),
            Ok(Ok(Err(e))) => Err(ApiError::new(ErrorCode::PmpUnavailable, e)),
            Ok(Err(_)) => Err(ApiError::new(ErrorCode::PmpUnavailable, "executor dropped")),
            Err(_) => Err(ApiError::new(ErrorCode::PmpUnavailable, "command timed out")),
        }
    } else {
        let accepted = json!({
            "command_id": command_id,
            "status": "queued",
        });
        Ok((StatusCode::ACCEPTED, Json(accepted)).into_response())
    }
}

// ── Admin room routes ──────────────────────────────────────────

async fn admin_list_rooms(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:view").await?;
    let result = state.rooms.list().await.map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct CreateRoomBody {
    pub room_id: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub persistent_empty: bool,
}

async fn admin_create_room(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateRoomBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:manage").await?;
    let result = state
        .rooms
        .create(&body.room_id, body.endpoint.as_deref(), body.persistent_empty)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn admin_room_info(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:view").await?;
    let result = state.rooms.info(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn admin_close_room(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:manage").await?;
    let result = state.rooms.close(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn room_banlist(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:blacklist").await?;
    let result = state.rooms.banlist(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn room_whitelist(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:whitelist").await?;
    let result = state.rooms.whitelist(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

// ── Helpers ────────────────────────────────────────────────────

async fn caller_phira_id(state: &Arc<AppState>, auth: &AuthPrincipal) -> Result<i32, ApiError> {
    let db = state.require_db()?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    Ok(user.phira_id as i32)
}

/// Re-verify the caller is the room's real host at execution time.
async fn verify_real_host(
    state: &Arc<AppState>,
    auth: &AuthPrincipal,
    args: &Value,
) -> Result<bool, ApiError> {
    let room_id = args
        .get("room_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::validation("room_id required for host action"))?;
    let phira_id = caller_phira_id(state, auth).await?;
    let host = state.rooms.host_id(room_id).await.map_err(ApiError::from)?;
    Ok(host == Some(phira_id))
}
