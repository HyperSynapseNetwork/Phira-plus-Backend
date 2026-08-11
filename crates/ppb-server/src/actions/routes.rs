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
use crate::commands::broker::{redact_args, CommandTask};
use crate::commands::repo as command_repo;
use crate::error::{ApiError, ErrorCode};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/actions", get(list_actions))
        .route("/actions/{action_id}/execute", post(execute_action))
        .route("/commands", get(list_commands))
}

#[derive(Debug, Deserialize)]
pub struct ExecuteActionBody {
    #[serde(default)]
    pub args: Value,
}

/// GET /api/v1/admin/actions — Action Manifest.
async fn list_actions(
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
async fn execute_action(
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
async fn list_commands(
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

/// Verify the caller is the room's real host at execution time.
///
/// Phase A scaffold: requires `room_id` in args and consults PMP `room.info`.
/// Until Phase B wires the typed host query, we conservatively return `false`
/// (deny) for host-only access; admins still pass via the permission gate.
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

    // TODO(Phase B): `room.info` → host_id == user.phira_id (plus server policy
    // flag for Web Host Control). Deny by default until wired.
    let _ = (room_id, user.phira_id);
    Ok(false)
}
