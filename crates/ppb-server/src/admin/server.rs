//! `/api/v1/admin/server/*` + broadcast routes.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorEnvelope};
use crate::pmp::openuds::client::OpenUdsError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/server/stats", get(server_stats))
        .route("/server/runtime", get(runtime_status))
        .route("/server/actions", post(server_actions))
        .route("/server/config-reload", post(config_reload))
        .route("/server/roomcreation", post(room_creation))
        .route("/server/shutdown", post(shutdown))
        .route("/server/broadcast/all", post(broadcast_all))
        .route("/server/broadcast/room", post(broadcast_room))
        .route("/server/broadcast/user", post(broadcast_user))
}

/// GET /api/v1/admin/server/stats — PMP server stats.
#[utoipa::path(
    get,
    path = "/api/v1/admin/server/stats",
    operation_id = "admin_server_stats_get",
    responses(
        (status = 200, description = "server stats", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn server_stats(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    let result = state.openuds.command("server.stats", json!({})).await.map_err(map_err)?;
    Ok(Json(result))
}

/// GET /api/v1/admin/server/runtime — runtime status.
#[utoipa::path(
    get,
    path = "/api/v1/admin/server/runtime",
    operation_id = "admin_server_runtime_get",
    responses(
        (status = 200, description = "runtime status", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn runtime_status(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    let result = state.openuds.command("runtime.status", json!({})).await.map_err(map_err)?;
    Ok(Json(result))
}

/// POST /api/v1/admin/server/config-reload — hot reload PMP config.
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/config-reload",
    operation_id = "admin_server_config_reload_post",
    responses(
        (status = 200, description = "config reloaded", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn config_reload(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    let result = state.openuds.command("server.config_reload", json!({})).await.map_err(map_err)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ServerActionBody {
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/admin/server/actions — unified server operation dispatch
/// (contract §17). config_reload / shutdown / roomcreation / connections.
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/actions",
    operation_id = "admin_server_actions_post",
    request_body = ServerActionBody,
    responses(
        (status = 200, description = "server action result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn server_actions(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ServerActionBody>,
) -> Result<Json<Value>, ApiError> {
    match body.action.as_str() {
        "config_reload" => {
            state.permissions.require(&state.db, &auth, "config:reload").await?;
            let r = state.openuds.command("server.config_reload", json!({})).await.map_err(map_err)?;
            Ok(Json(r))
        }
        "shutdown" => {
            state.permissions.require(&state.db, &auth, "server:shutdown").await?;
            check_reauth_header(&state, &auth, &headers, ReauthRisk::Critical)?;
            let r = state.openuds.command("server.shutdown", json!({})).await.map_err(map_err)?;
            Ok(Json(r))
        }
        "roomcreation" => {
            state.permissions.require(&state.db, &auth, "server:manage").await?;
            let enabled = body.args.get("enabled").and_then(Value::as_bool).unwrap_or(false);
            let r = state.openuds.command("server.roomcreation", json!({ "enabled": enabled })).await.map_err(map_err)?;
            Ok(Json(r))
        }
        "connections" => {
            state.permissions.require(&state.db, &auth, "server:view").await?;
            let r = state.openuds.command("runtime.status", json!({})).await.map_err(map_err)?;
            Ok(Json(r))
        }
        other => Err(ApiError::validation(format!("unknown server action: {other}"))),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RoomCreationBody {
    pub enabled: bool,
}

/// POST /api/v1/admin/server/roomcreation — toggle room creation gate.
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/roomcreation",
    operation_id = "admin_server_roomcreation_post",
    request_body = RoomCreationBody,
    responses(
        (status = 200, description = "gate toggled", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn room_creation(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RoomCreationBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:manage").await?;
    let result = state
        .openuds
        .command("server.roomcreation", json!({ "enabled": body.enabled }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

/// POST /api/v1/admin/server/shutdown — shutdown PMP (critical reauth).
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/shutdown",
    operation_id = "admin_server_shutdown_post",
    responses(
        (status = 200, description = "shutdown initiated", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn shutdown(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:shutdown").await?;
    // P-80: every server.shutdown path requires reauth (critical).
    check_reauth_header(&state, &auth, &headers, ReauthRisk::Critical)?;
    let result = state.openuds.command("server.shutdown", json!({})).await.map_err(map_err)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BroadcastBody {
    pub content: String,
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<i64>,
}

/// POST /api/v1/admin/server/broadcast/all — server-wide system broadcast.
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/broadcast/all",
    operation_id = "admin_server_broadcast_all_post",
    request_body = BroadcastBody,
    responses(
        (status = 200, description = "broadcast sent", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn broadcast_all(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<BroadcastBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "broadcast:all").await?;
    let result = state
        .openuds
        .command("broadcast.all", json!({ "message": body.content }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

/// POST /api/v1/admin/server/broadcast/room — room system broadcast.
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/broadcast/room",
    operation_id = "admin_server_broadcast_room_post",
    request_body = BroadcastBody,
    responses(
        (status = 200, description = "broadcast sent", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn broadcast_room(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<BroadcastBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "broadcast:room").await?;
    let room_id = body
        .room_id
        .ok_or_else(|| ApiError::validation("room_id required for broadcast.room"))?;
    let result = state
        .openuds
        .command("broadcast.room", json!({ "room_id": room_id, "message": body.content }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

/// POST /api/v1/admin/server/broadcast/user — user system message.
#[utoipa::path(
    post,
    path = "/api/v1/admin/server/broadcast/user",
    operation_id = "admin_server_broadcast_user_post",
    request_body = BroadcastBody,
    responses(
        (status = 200, description = "broadcast sent", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn broadcast_user(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<BroadcastBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "broadcast:user").await?;
    let user_id = body
        .user_id
        .ok_or_else(|| ApiError::validation("user_id required for broadcast.user"))?;
    let result = state
        .openuds
        .command("broadcast.user", json!({ "user_id": user_id, "message": body.content }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

fn map_err(e: OpenUdsError) -> ApiError {
    ApiError::from(e)
}
