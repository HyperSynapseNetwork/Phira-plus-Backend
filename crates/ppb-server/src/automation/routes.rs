//! `/api/v1/admin/automation` runbooks (design §10, contract §17).
//!
//! V1: runbook CRUD + `POST /runbooks/{id}/run` + `GET /runbook-runs`.
//! No shell executor; each step references a registered Action and is
//! re-authorized against the current principal's permissions.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::FromRow;
use tokio::sync::oneshot;
use uuid::Uuid;

use super::{validate_steps, RunbookDefinition};
use crate::actions::types::Risk;
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
        .route("/runbooks", get(list).post(create))
        .route("/runbooks/{id}", get(get_one).patch(update).delete(delete_runbook))
        .route("/runbooks/{id}/run", post(run))
        .route("/runbook-runs", get(runs))
        .route("/runbook-runs/{id}", get(get_run))
        .route("/runbook-runs/{id}/cancel", post(cancel_run))
}

/// GET /api/v1/admin/automation/runbook-runs/{id} — single run detail.
#[utoipa::path(
    get,
    path = "/api/v1/admin/automation/runbook-runs/{id}",
    responses(
        (status = 200, description = "runbook run detail", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn get_run(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunbookRunRow>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:view").await?;
    let db = state.require_db()?;
    let row = sqlx::query_as::<_, RunbookRunRow>(
        "SELECT id, runbook_id, definition_snapshot, arguments_redacted, actor, status, started_at, finished_at
         FROM runbook_runs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| ApiError::not_found("runbook run"))?;
    Ok(Json(row))
}

/// POST /api/v1/admin/automation/runbook-runs/{id}/cancel — cancel a queued/running run.
#[utoipa::path(
    post,
    path = "/api/v1/admin/automation/runbook-runs/{id}/cancel",
    responses(
        (status = 200, description = "cancelled", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn cancel_run(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:execute").await?;
    let db = state.require_db()?;
    sqlx::query(
        "UPDATE runbook_runs SET status = 'cancelled', finished_at = now()
         WHERE id = $1 AND status IN ('queued', 'running')",
    )
    .bind(id)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(Json(json!({ "cancelled": id })))
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RunbookRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub definition: Value,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct RunbookRunRow {
    pub id: Uuid,
    pub runbook_id: Uuid,
    pub definition_snapshot: Value,
    pub arguments_redacted: Value,
    pub actor: String,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateRunbookBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub definition: RunbookDefinition,
}

const RUNBOOK_SELECT: &str =
    "SELECT id, name, description, definition, created_by, created_at, updated_at FROM runbooks";

async fn fetch_runbook(db: &sqlx::PgPool, id: Uuid) -> Result<RunbookRow, ApiError> {
    sqlx::query_as::<_, RunbookRow>(&format!("{RUNBOOK_SELECT} WHERE id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::not_found("runbook"))
}

/// POST /api/v1/admin/automation/runbooks — create a runbook.
#[utoipa::path(
    post,
    path = "/api/v1/admin/automation/runbooks",
    request_body = CreateRunbookBody,
    responses(
        (status = 200, description = "runbook created", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn create(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateRunbookBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:edit").await?;
    validate_steps(&body.definition.steps, &state.actions).map_err(ApiError::validation)?;
    let db = state.require_db()?;
    let def = serde_json::to_value(&body.definition).map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
    let row = sqlx::query_as::<_, RunbookRow>(
        "INSERT INTO runbooks (name, description, definition, created_by, updated_by)
         VALUES ($1, $2, $3, $4, $4)
         RETURNING id, name, description, definition, created_by, created_at, updated_at",
    )
    .bind(body.name.trim())
    .bind(&body.description)
    .bind(def)
    .bind(auth.sub)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    Ok(Json(serde_json::to_value(row).unwrap_or(Value::Null)))
}

/// GET /api/v1/admin/automation/runbooks — list runbooks.
#[utoipa::path(
    get,
    path = "/api/v1/admin/automation/runbooks",
    responses(
        (status = 200, description = "runbook list", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<RunbookRow>>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:view").await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, RunbookRow>(RUNBOOK_SELECT)
        .fetch_all(db)
        .await
        .map_err(db_err)?;
    Ok(Json(rows))
}

/// GET /api/v1/admin/automation/runbooks/{id} — runbook detail.
#[utoipa::path(
    get,
    path = "/api/v1/admin/automation/runbooks/{id}",
    responses(
        (status = 200, description = "runbook detail", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn get_one(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<RunbookRow>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:view").await?;
    let db = state.require_db()?;
    Ok(Json(fetch_runbook(db, id).await?))
}

/// PATCH /api/v1/admin/automation/runbooks/{id} — update a runbook.
#[utoipa::path(
    patch,
    path = "/api/v1/admin/automation/runbooks/{id}",
    request_body = CreateRunbookBody,
    responses(
        (status = 200, description = "runbook updated", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateRunbookBody>,
) -> Result<Json<RunbookRow>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:edit").await?;
    validate_steps(&body.definition.steps, &state.actions).map_err(ApiError::validation)?;
    let db = state.require_db()?;
    let def = serde_json::to_value(&body.definition).map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;
    let row = sqlx::query_as::<_, RunbookRow>(
        "UPDATE runbooks SET name = $1, description = $2, definition = $3, updated_by = $4, updated_at = now()
         WHERE id = $5
         RETURNING id, name, description, definition, created_by, created_at, updated_at",
    )
    .bind(body.name.trim())
    .bind(&body.description)
    .bind(def)
    .bind(auth.sub)
    .bind(id)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    Ok(Json(row))
}

/// DELETE /api/v1/admin/automation/runbooks/{id} — delete a runbook.
#[utoipa::path(
    delete,
    path = "/api/v1/admin/automation/runbooks/{id}",
    responses(
        (status = 204, description = "deleted"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn delete_runbook(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:edit").await?;
    let db = state.require_db()?;
    sqlx::query("DELETE FROM runbooks WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct RunBody {
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/admin/automation/runbooks/{id}/run — snapshot + execute steps sequentially.
#[utoipa::path(
    post,
    path = "/api/v1/admin/automation/runbooks/{id}/run",
    responses(
        (status = 200, description = "run result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn run(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Path(id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:execute").await?;
    let db = state.require_db()?;
    let runbook = fetch_runbook(db, id).await?;
    let definition: RunbookDefinition =
        serde_json::from_value(runbook.definition.clone()).map_err(|e| ApiError::validation(format!("invalid definition: {e}")))?;
    validate_steps(&definition.steps, &state.actions).map_err(ApiError::validation)?;

    let run_args = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<RunBody>(&body)
            .map_err(|e| ApiError::validation(format!("invalid body: {e}")))?
            .args
    };
    let run_id = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO runbook_runs (runbook_id, definition_snapshot, arguments_redacted, actor, status)
         VALUES ($1, $2, $3, $4, 'running')
         RETURNING id",
    )
    .bind(id)
    .bind(runbook.definition.clone())
    .bind(run_args.clone())
    .bind(auth.sub.to_string())
    .fetch_one(db)
    .await
    .map_err(db_err)?
    .0;

    let mut results: Vec<Value> = Vec::new();
    let mut ok = true;
    for step in &definition.steps {
        // WAIT-only step (design §10.1): no action, just sleep.
        if step.action.is_empty() {
            if let Some(wait) = step.wait_secs {
                results.push(json!({ "step": "wait", "ok": true, "wait_secs": wait }));
                tokio::time::sleep(Duration::from_secs(wait.min(3600))).await;
            }
            continue;
        }
        let action = state
            .actions
            .get(&step.action)
            .ok_or_else(|| ApiError::not_found("action"))?;
        // Re-authorize each step (design §10).
        if !state.permissions.has_permission(&state.db, &auth, action.permission).await? {
            ok = false;
            results.push(json!({ "step": step.action, "ok": false, "error": "permission_denied" }));
            break;
        }
        if action.reauth {
            let risk = if action.risk >= Risk::Critical { ReauthRisk::Critical } else { ReauthRisk::High };
            check_reauth_header(&state, &auth, &headers, risk)?;
        }
        let mut args = step.with.clone();
        if let (Value::Object(map), Value::Object(run)) = (&mut args, &run_args) {
            for (k, v) in run {
                map.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        let queue_key = state.actions.resolve_queue_key(action, &args);
        let command_id = Uuid::new_v4();
        let args_redacted = redact_args(&args);
        command_repo::insert_queued(db, command_id, action.id, &auth.sub.to_string(), &queue_key, args_redacted.clone())
            .await?;
        // Gate 0 A5: each step is recorded by the executor with its FINAL result.
        let audit = if action.audit {
            Some(CommandAudit {
                principal_type: auth.principal_type.to_string(),
                actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
                actor_session_id: auth.sid,
                action: action.id.to_string(),
                resource_type: "runbook".to_string(),
                resource_id: id.to_string(),
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
            Ok(Ok(Ok(v))) => results.push(json!({ "step": step.action, "ok": true, "result": v })),
            _ => {
                ok = false;
                results.push(json!({ "step": step.action, "ok": false, "error": "command failed" }));
                break;
            }
        }
        if let Some(wait) = step.wait_secs {
            tokio::time::sleep(Duration::from_secs(wait.min(3600))).await;
        }
    }

    let status = if ok { "succeeded" } else { "failed" };
    sqlx::query("UPDATE runbook_runs SET status = $1, finished_at = now() WHERE id = $2")
        .bind(status)
        .bind(run_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(Json(json!({ "run_id": run_id, "status": status, "results": results })))
}

/// GET /api/v1/admin/automation/runbook-runs — recent runbook runs.
#[utoipa::path(
    get,
    path = "/api/v1/admin/automation/runbook-runs",
    responses(
        (status = 200, description = "runbook run list", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn runs(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<RunbookRunRow>>, ApiError> {
    state.permissions.require(&state.db, &auth, "automation:view").await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, RunbookRunRow>(
        "SELECT id, runbook_id, definition_snapshot, arguments_redacted, actor, status, started_at, finished_at
         FROM runbook_runs ORDER BY started_at DESC NULLS LAST LIMIT 200",
    )
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(Json(rows))
}

fn ip_from_headers(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

fn user_agent_from_headers(headers: &axum::http::HeaderMap) -> String {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "automation db error");
        ApiError::internal()
    }
}
