//! Room REST routes: `/api/v1/rooms/*` (public + host) and `/api/v1/admin/rooms/*`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::actions::types::Risk;
use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
use crate::commands::broker::{redact_args, CommandAudit, CommandTask};
use crate::commands::repo as command_repo;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};
// `use crate::error::ErrorEnvelope` is referenced by utoipa path macros below.

/// Public + logged-in room routes.
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rooms", get(list_rooms))
        .route("/rooms/{room_id}", get(room_info))
        .route("/rooms/{room_id}/history", get(room_history))
        .route("/rooms/{room_id}/chat-history", get(room_chat_history))
        .route("/rooms/{room_id}/chat", get(room_chat_history).post(send_chat))
        .route("/rooms/{room_id}/actions", post(room_action_body))
        .route("/rooms/{room_id}/actions/{action}", post(room_action))
}

/// Admin room routes (permission-gated superset).
pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rooms", get(admin_list_rooms).post(admin_create_room))
        .route("/rooms/{room_id}", get(admin_room_info).delete(admin_close_room))
        .route("/rooms/{room_id}/actions", post(admin_room_action))
        .route("/rooms/actions/batch", post(admin_room_actions_batch))
        .route("/rooms/{room_id}/banlist", get(room_banlist))
        .route("/rooms/{room_id}/whitelist", get(room_whitelist))
}

// ── Public / host routes ───────────────────────────────────────

/// GET /api/v1/rooms — list rooms (is_self enrichment when authenticated).
#[utoipa::path(
    get,
    path = "/api/v1/rooms",
    operation_id = "rooms_get",
    responses(
        (status = 200, description = "room list", body = RoomListResponse),
        (status = 502, description = "pmp unavailable", body = ErrorEnvelope),
    ),
    tag = "rooms"
)]
pub async fn list_rooms(
    auth: crate::middleware::auth::OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<RoomListParams>,
) -> Result<Json<RoomListResponse>, ApiError> {
    let (page, page_num) = resolve_page(params.page, params.page_num)?;
    let (rooms, total) = state.rooms.list().await.map_err(ApiError::from)?;
    let my_phira = caller_phira_id_opt(&state, auth.0).await?;
    let items = paginate_room_items(rooms, page, page_num, my_phira);
    Ok(Json(RoomListResponse {
        items,
        total,
        page,
        page_num,
    }))
}

/// GET /api/v1/rooms/{room_id} — room detail (is_self enrichment).
#[utoipa::path(
    get,
    path = "/api/v1/rooms/{room_id}",
    operation_id = "rooms_room_id_get",
    responses(
        (status = 200, description = "room detail", body = serde_json::Value),
        (status = 404, description = "room not found", body = ErrorEnvelope),
    ),
    tag = "rooms"
)]
pub async fn room_info(
    auth: crate::middleware::auth::OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let mut result = state.rooms.info(&room_id).await.map_err(ApiError::from)?;
    let my_phira = caller_phira_id_opt(&state, auth.0).await?;
    enrich_room_is_self(&mut result, my_phira);
    Ok(Json(result))
}

/// GET /api/v1/rooms/{room_id}/history — PMP room.history (rounds + scores).
#[utoipa::path(
    get,
    path = "/api/v1/rooms/{room_id}/history",
    operation_id = "rooms_room_id_history_get",
    responses(
        (status = 200, description = "room rounds + scores", body = serde_json::Value),
        (status = 502, description = "pmp unavailable", body = ErrorEnvelope),
    ),
    tag = "rooms"
)]
pub async fn room_history(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let result = state.rooms.history(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

/// GET /api/v1/rooms/{room_id}/chat — room chat history.
#[utoipa::path(
    get,
    path = "/api/v1/rooms/{room_id}/chat",
    operation_id = "rooms_room_id_chat_get",
    responses(
        (status = 200, description = "chat history", body = serde_json::Value),
        (status = 502, description = "pmp unavailable", body = ErrorEnvelope),
    ),
    tag = "rooms"
)]
pub async fn room_chat_history(
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let result = state.rooms.chat_history(&room_id, None).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ChatSendBody {
    pub content: String,
}

/// POST /api/v1/rooms/{room_id}/chat — send a room chat message as the caller.
/// Client must not specify a trusted user_id (design §13.3).
#[utoipa::path(
    post,
    path = "/api/v1/rooms/{room_id}/chat",
    operation_id = "rooms_room_id_chat_post",
    request_body = ChatSendBody,
    responses(
        (status = 200, description = "chat sent", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "rooms"
)]
pub async fn send_chat(
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
        .check(&format!("chat-send:{room_id}:{}", auth.sub), state.config.rate_limit.chat_send_per_minute)?;
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoomActionBody2 {
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/rooms/{room_id}/actions — contract §18 body form `{action, args}`.
#[utoipa::path(
    post,
    path = "/api/v1/rooms/{room_id}/actions",
    operation_id = "rooms_room_id_actions_post",
    request_body = RoomActionBody2,
    responses(
        (status = 200, description = "action result", body = serde_json::Value),
        (status = 202, description = "long-running accepted", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "rooms"
)]
pub async fn room_action_body(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(body): Json<RoomActionBody2>,
) -> Result<axum::response::Response, ApiError> {
    execute_room_action(&state, &auth, &headers, &room_id, &body.action, body.args).await
}

/// POST /api/v1/rooms/{room_id}/actions/{action} — host-or-permission action.
async fn room_action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((room_id, action_id)): Path<(String, String)>,
    Json(body): Json<RoomActionBody>,
) -> Result<axum::response::Response, ApiError> {
    execute_room_action(&state, &auth, &headers, &room_id, &action_id, body.args).await
}

/// Shared room-action execution (contract §18: host resolved from the Session,
/// real host re-checked at execution time; room.kick target = `args.phira_id`,
/// room.set_chart target = `args.chart_id`).
async fn execute_room_action(
    state: &Arc<AppState>,
    auth: &AuthPrincipal,
    headers: &HeaderMap,
    room_id: &str,
    action_id: &str,
    body_args: Value,
) -> Result<axum::response::Response, ApiError> {
    let action = state
        .actions
        .get(action_id)
        .ok_or_else(|| ApiError::not_found("action"))?;

    // Merge room_id into args (host/resource checks need it).
    let mut args = body_args;
    if args.get("room_id").is_none() {
        if let Value::Object(map) = &mut args {
            map.insert("room_id".to_string(), json!(room_id));
        }
    }

    // Authorization: admin permission OR (host_allowed AND real host).
    let has_permission = state
        .permissions
        .has_permission(&state.db, auth, action.permission)
        .await?;
    let is_host = if action.host_allowed {
        verify_real_host(state, auth, &args).await?
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
        check_reauth_header(state, auth, headers, risk)?;
    }

    let db = state.require_db()?;
    let queue_key = state.actions.resolve_queue_key(action, &args);
    let command_id = Uuid::new_v4();
    let args_redacted = redact_args(&args);
    command_repo::insert_queued(db, command_id, action.id, &auth.sub.to_string(), &queue_key, args_redacted.clone())
        .await?;

    // Gate 0 A5: audited actions are recorded by the executor with the FINAL
    // result once the command completes — no pre-recorded success.
    let audit = if action.audit {
        Some(CommandAudit {
            principal_type: auth.principal_type.to_string(),
            actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
            actor_session_id: auth.sid,
            action: action.id.to_string(),
            resource_type: "action".to_string(),
            resource_id: queue_key.clone(),
            request_id: auth.request_id.clone(),
            ip: ip_from_headers(headers),
            user_agent: user_agent_from_headers(headers),
        })
    } else {
        None
    };

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
            audit,
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

/// GET /api/v1/admin/rooms — list rooms (admin superset).
#[utoipa::path(
    get,
    path = "/api/v1/admin/rooms",
    operation_id = "admin_rooms_get",
    responses(
        (status = 200, description = "room list", body = RoomListResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn admin_list_rooms(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<RoomListParams>,
) -> Result<Json<RoomListResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:view").await?;
    let (page, page_num) = resolve_page(params.page, params.page_num)?;
    let (rooms, total) = state.rooms.list().await.map_err(ApiError::from)?;
    let items = paginate_room_items(rooms, page, page_num, None);
    Ok(Json(RoomListResponse {
        items,
        total,
        page,
        page_num,
    }))
}

/// Query params for room list endpoints (contract §22: `page` 1-based,
/// `pageNum` ≤ 100; rooms default to 50 per page).
#[derive(Debug, Deserialize)]
pub struct RoomListParams {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default, rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// Paginated room list response (§22 `{items, total, page, pageNum}`).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RoomListResponse {
    pub items: Vec<Value>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRoomBody {
    pub room_id: String,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub persistent_empty: bool,
}

/// POST /api/v1/admin/rooms — create a room.
#[utoipa::path(
    post,
    path = "/api/v1/admin/rooms",
    operation_id = "admin_rooms_post",
    request_body = CreateRoomBody,
    responses(
        (status = 200, description = "room created", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn admin_create_room(
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

/// GET /api/v1/admin/rooms/{room_id} — room detail (admin).
#[utoipa::path(
    get,
    path = "/api/v1/admin/rooms/{room_id}",
    operation_id = "admin_rooms_room_id_get",
    responses(
        (status = 200, description = "room detail", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn admin_room_info(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(room_id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "room:view").await?;
    let result = state.rooms.info(&room_id).await.map_err(ApiError::from)?;
    Ok(Json(result))
}

/// DELETE /api/v1/admin/rooms/{room_id} — close a room.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/rooms/{room_id}",
    operation_id = "admin_rooms_room_id_delete",
    responses(
        (status = 200, description = "room closed", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn admin_close_room(
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

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AdminRoomActionBody {
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/admin/rooms/{room_id}/actions — run a registered action scoped
/// to a room (admin-gated or host-allowed, re-derived host).
#[utoipa::path(
    post,
    path = "/api/v1/admin/rooms/{room_id}/actions",
    operation_id = "admin_rooms_room_id_actions_post",
    request_body = AdminRoomActionBody,
    responses(
        (status = 200, description = "action result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn admin_room_action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(room_id): Path<String>,
    Json(body): Json<AdminRoomActionBody>,
) -> Result<axum::response::Response, ApiError> {
    let action = state
        .actions
        .get(&body.action)
        .ok_or_else(|| ApiError::not_found("action"))?;
    let mut args = body.args;
    if args.get("room_id").is_none() {
        if let Value::Object(map) = &mut args {
            map.insert("room_id".to_string(), json!(room_id));
        }
    }

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
    // Gate 0 A5: audited actions are recorded by the executor with the FINAL
    // result once the command completes — no pre-recorded success.
    let audit = if action.audit {
        Some(CommandAudit {
            principal_type: auth.principal_type.to_string(),
            actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
            actor_session_id: auth.sid,
            action: action.id.to_string(),
            resource_type: "room".to_string(),
            resource_id: room_id.clone(),
            request_id: auth.request_id.clone(),
            ip: ip_from_headers(&headers),
            user_agent: user_agent_from_headers(&headers),
        })
    } else {
        None
    };

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
            audit,
        })?;
    if let Some(rx) = rx {
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(v))) => Ok(Json(v).into_response()),
            Ok(Ok(Err(e))) => Err(ApiError::new(ErrorCode::PmpUnavailable, e)),
            Ok(Err(_)) => Err(ApiError::new(ErrorCode::PmpUnavailable, "executor dropped")),
            Err(_) => Err(ApiError::new(ErrorCode::PmpUnavailable, "command timed out")),
        }
    } else {
        let accepted = json!({ "command_id": command_id, "status": "queued" });
        Ok((StatusCode::ACCEPTED, Json(accepted)).into_response())
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoomBatchBody {
    pub action: String,
    pub room_ids: Vec<String>,
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/admin/rooms/actions/batch — batch room action (kick/move/ban)
/// with per-item results and partial failure.
#[utoipa::path(
    post,
    path = "/api/v1/admin/rooms/actions/batch",
    operation_id = "admin_rooms_actions_batch_post",
    request_body = RoomBatchBody,
    responses(
        (status = 200, description = "per-item results", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn admin_room_actions_batch(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RoomBatchBody>,
) -> Result<Json<Value>, ApiError> {
    let action = state
        .actions
        .get(&body.action)
        .ok_or_else(|| ApiError::not_found("action"))?;
    // Batch is limited to clearly safe room actions (design §18.3).
    if !matches!(action.id, "room.kick" | "room.force_move" | "room.ban" | "room.unban") {
        return Err(ApiError::validation("batch only supports kick/move/ban"));
    }
    state
        .permissions
        .require(&state.db, &auth, action.permission)
        .await?;
    if action.reauth {
        let risk = if action.risk >= Risk::Critical {
            ReauthRisk::Critical
        } else {
            ReauthRisk::High
        };
        check_reauth_header(&state, &auth, &headers, risk)?;
    }

    let mut results: Vec<Value> = Vec::new();
    let mut succeeded = 0i64;
    let mut failed = 0i64;
    for room_id in &body.room_ids {
        let mut args = body.args.clone();
        if let Value::Object(map) = &mut args {
            map.insert("room_id".to_string(), json!(room_id));
        }
        let db = state.require_db()?;
        let queue_key = state.actions.resolve_queue_key(action, &args);
        let command_id = Uuid::new_v4();
        let args_redacted = redact_args(&args);
        command_repo::insert_queued(db, command_id, action.id, &auth.sub.to_string(), &queue_key, args_redacted.clone())
            .await?;
        // Gate 0 A5: each audited batch item is recorded by the executor with
        // its FINAL result — no pre-recorded success.
        let audit = if action.audit {
            Some(CommandAudit {
                principal_type: auth.principal_type.to_string(),
                actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
                actor_session_id: auth.sid,
                action: action.id.to_string(),
                resource_type: "room".to_string(),
                resource_id: room_id.clone(),
                request_id: auth.request_id.clone(),
                ip: ip_from_headers(&headers),
                user_agent: user_agent_from_headers(&headers),
            })
        } else {
            None
        };
        let (tx, rx) = oneshot::channel();
        state
            .commands
            .submit(CommandTask {
                command_id,
                action: action.id.to_string(),
                actor: auth.sub.to_string(),
                resource_key: queue_key,
                args,
                args_redacted,
                completion: Some(tx),
                audit,
            })?;
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(v))) => {
                succeeded += 1;
                results.push(json!({ "room_id": room_id, "ok": true, "result": v }));
            }
            Ok(Ok(Err(_))) | Ok(Err(_)) | Err(_) => {
                failed += 1;
                results.push(json!({ "room_id": room_id, "ok": false, "error": "command failed" }));
            }
        }
    }
    Ok(Json(json!({ "items": results, "succeeded": succeeded, "failed": failed })))
}

// ── Helpers ────────────────────────────────────────────────────

async fn caller_phira_id(state: &Arc<AppState>, auth: &AuthPrincipal) -> Result<i32, ApiError> {
    let db = state.require_db()?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    Ok(user.phira_id as i32)
}

/// Optional caller phira_id for `is_self` enrichment (None for anonymous/root).
async fn caller_phira_id_opt(
    state: &Arc<AppState>,
    auth: Option<AuthPrincipal>,
) -> Result<Option<i64>, ApiError> {
    let Some(auth) = auth else { return Ok(None) };
    if auth.is_root() {
        return Ok(None);
    }
    let Some(db) = &state.db else { return Ok(None) };
    let user = crate::users::repo::find_by_id(db, auth.sub).await?;
    Ok(user.map(|u| u.phira_id))
}

/// Add `host.is_self` and `players[].is_self` to a room.info payload (contract §18).
fn enrich_room_is_self(value: &mut Value, my_phira: Option<i64>) {
    let Some(my_phira) = my_phira else { return };
    if let Some(host) = value.get_mut("host") {
        if host.is_object() {
            let hid = host
                .get("user_id")
                .and_then(Value::as_i64)
                .or_else(|| host.get("id").and_then(Value::as_i64));
            if let Some(hid) = hid {
                host["is_self"] = json!(hid == my_phira);
            }
        }
    } else if let Some(hid) = value.get("host_id").and_then(Value::as_i64) {
        if let Some(map) = value.as_object_mut() {
            map.insert("host".to_string(), json!({ "user_id": hid, "is_self": hid == my_phira }));
        }
    }
    if let Some(players) = value.get_mut("players").and_then(Value::as_array_mut) {
        for p in players {
            if let Some(pid) = p
                .get("user_id")
                .and_then(Value::as_i64)
                .or_else(|| p.get("id").and_then(Value::as_i64))
            {
                p["is_self"] = json!(pid == my_phira);
            }
        }
    }
}

/// Resolve room-list pagination: `page` (1-based) and `pageNum` (≤100, default 50).
fn resolve_page(page: Option<i64>, page_num: Option<i64>) -> Result<(i64, i64), ApiError> {
    let page = page.unwrap_or(1).max(1);
    let page_num = page_num.unwrap_or(50);
    if !(1..=100).contains(&page_num) {
        return Err(ApiError::validation("pageNum must be between 1 and 100"));
    }
    Ok((page, page_num))
}

/// Slice the in-memory room list to the requested page, applying optional
/// `is_self` enrichment to each item.
fn paginate_room_items(
    rooms: Vec<Value>,
    page: i64,
    page_num: i64,
    my_phira: Option<i64>,
) -> Vec<Value> {
    let start = ((page - 1) * page_num) as usize;
    let mut items: Vec<Value> = rooms.into_iter().skip(start).take(page_num as usize).collect();
    for room in &mut items {
        enrich_room_is_self(room, my_phira);
    }
    items
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

fn ip_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

fn user_agent_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}
