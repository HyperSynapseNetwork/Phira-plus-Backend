//! Admin Action routes: manifest + execute (design §9).

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

use super::types::{ActionDescriptor, Risk};
use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
use crate::commands::broker::{redact_args, CommandAudit, CommandTask};
use crate::commands::repo as command_repo;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/actions", get(list_actions))
        .route("/actions/{action_id}/execute", post(execute_action))
        .route("/commands", get(list_commands))
        .route("/commands/history", get(list_commands))
        .route("/commands/execute", post(execute_command))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExecuteActionBody {
    #[serde(default)]
    pub args: Value,
}

/// GET /api/v1/admin/actions — Action Manifest.
#[utoipa::path(
    get,
    path = "/api/v1/admin/actions",
    responses(
        (status = 200, description = "action manifest", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list_actions(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<&'static ActionDescriptor>>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "dashboard:view")
        .await?;
    let mut actions = state.actions.all();
    actions.sort_by_key(|a| a.id);
    Ok(Json(actions))
}

/// POST /api/v1/admin/actions/{id}/execute
#[utoipa::path(
    post,
    path = "/api/v1/admin/actions/{action_id}/execute",
    request_body = ExecuteActionBody,
    responses(
        (status = 200, description = "action result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn execute_action(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(action_id): Path<String>,
    Json(body): Json<ExecuteActionBody>,
) -> Result<axum::response::Response, ApiError> {
    let action = state
        .actions
        .get(&action_id)
        .ok_or_else(|| ApiError::not_found("action"))?;

    // 1) Authorization (RBAC + ABAC/resource policy, design §8.5):
    //    permission granted  OR  (host_allowed action AND caller is the real host).
    let has_permission = state
        .permissions
        .has_permission(&state.db, &auth, action.permission)
        .await?;
    if action.host_allowed {
        // Host relationship is re-derived at execution time — never trust client flags.
        let is_host = verify_real_host(&state, &auth, &body.args).await?;
        if !has_permission && !is_host {
            return Err(ApiError::permission_denied());
        }
    } else if !has_permission {
        return Err(ApiError::permission_denied());
    }

    // 2) Reauth gate for high/critical actions marked reauth.
    if action.reauth {
        let risk = if action.risk >= Risk::Critical {
            ReauthRisk::Critical
        } else {
            ReauthRisk::High
        };
        check_reauth_header(&state, &auth, &headers, risk)?;
    }

    let db = state.require_db()?;
    let queue_key = state.actions.resolve_queue_key(action, &body.args);
    let command_id = Uuid::new_v4();
    let args_redacted = redact_args(&body.args);
    command_repo::insert_queued(
        db,
        command_id,
        action.id,
        &auth.sub.to_string(),
        &queue_key,
        args_redacted.clone(),
    )
    .await?;

    // Gate 0 A5: audited actions are recorded by the executor with the FINAL
    // result once the command completes (success/failure/timeout). We only
    // attach the audit metadata here — no pre-recorded `success`.
    let audit = if action.audit {
        Some(CommandAudit {
            principal_type: auth.principal_type.to_string(),
            actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
            actor_session_id: auth.sid,
            action: action.id.to_string(),
            resource_type: "action".to_string(),
            resource_id: queue_key.clone(),
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

    let task = CommandTask {
        command_id,
        action: action.id.to_string(),
        actor: auth.sub.to_string(),
        resource_key: queue_key,
        args: body.args,
        args_redacted,
        completion,
        audit,
    };
    state.commands.submit(task)?;

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
            "message": "long-running job accepted",
        });
        Ok((StatusCode::ACCEPTED, Json(accepted)).into_response())
    }
}

/// GET /api/v1/admin/commands — recent command runs.
#[utoipa::path(
    get,
    path = "/api/v1/admin/commands",
    responses(
        (status = 200, description = "command runs", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list_commands(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::commands::model::CommandRun>>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "dashboard:view")
        .await?;
    let db = state.require_db()?;
    let runs = command_repo::list_recent(db, 100).await?;
    Ok(Json(runs))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ExecuteCommandBody {
    pub command: String,
}

/// POST /api/v1/admin/commands/execute — raw PMP console command (contract §17).
/// Requires an elevated reauth context and is fully audited with the **final**
/// result (success / failure / timeout) — never a pre-recorded success.
#[utoipa::path(
    post,
    path = "/api/v1/admin/commands/execute",
    request_body = ExecuteCommandBody,
    responses(
        (status = 200, description = "command result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn execute_command(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ExecuteCommandBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "pmp:cli").await?;
    state
        .rate_limiter
        .check(&format!("raw-cli:{}", auth.sub), state.config.rate_limit.raw_cli_per_minute)?;
    // Gate 0 A3: raw console requires an elevated reauth context.
    check_reauth_header(&state, &auth, &headers, ReauthRisk::High)?;

    let (result, result_status, error_code) = match tokio::time::timeout(
        Duration::from_secs(30),
        crate::pmp::cli::cli_execute(&state.openuds, &body.command),
    )
    .await
    {
        Ok(Ok(v)) => (Ok(v), "succeeded", String::new()),
        Ok(Err(e)) => (
            Err(ApiError::from(e)),
            "failed",
            "cli_execute_error".to_string(),
        ),
        Err(_) => (
            Err(ApiError::new(ErrorCode::PmpUnavailable, "command timed out")),
            "timeout",
            "timeout".to_string(),
        ),
    };

    // Full audit with the terminal outcome (success / failure / timeout).
    if let Some(db) = &state.db {
        let _ = crate::audit::service::record_principal(
            db,
            &auth,
            "pmp.cli.execute",
            "pmp",
            "console",
            serde_json::json!({ "command": "[REDACTED input]" }),
            result_status,
            &error_code,
            "",
            &ip_from_headers(&headers),
            &user_agent_from_headers(&headers),
        )
        .await;
    }
    Ok(Json(result?))
}

/// Verify the caller is the room's real host at execution time (design §8.5).
///
/// Re-queries PMP `room.info` → `host_id` and compares with the caller's
/// phira_id. Never trusts a client-supplied host flag.
async fn verify_real_host(
    state: &Arc<AppState>,
    auth: &AuthPrincipal,
    args: &Value,
) -> Result<bool, ApiError> {
    let room_id = args
        .get("room_id")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::validation("room_id required for host action"))?;
    let db = state.require_db()?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    let host = state.rooms.host_id(room_id).await.map_err(ApiError::from)?;
    Ok(host == Some(user.phira_id as i32))
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
