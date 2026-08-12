//! `/api/v1/admin/jobs/*` routes.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::Job;
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/jobs", get(list).post(create))
        .route("/jobs/{job_id}", get(get_job))
        .route("/jobs/{job_id}/cancel", post(cancel))
        .route("/jobs/{job_id}/retry", post(retry_job))
        .route("/jobs/tasks", get(list_tasks))
        .route("/jobs/tasks/{task_id}/complete", post(complete_task))
}

/// POST /api/v1/admin/jobs/{job_id}/retry — re-queue a failed/cancelled job.
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs/{job_id}/retry",
    responses(
        (status = 200, description = "job re-queued", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn retry_job(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:update").await?;
    state.jobs.retry(job_id).await?;
    Ok(Json(json!({ "ok": true, "job_id": job_id })))
}

/// GET /api/v1/admin/jobs/tasks — manual admin tasks (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/admin/jobs/tasks",
    responses(
        (status = 200, description = "admin tasks", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list_tasks(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "coupon:view").await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Value, Option<Uuid>, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT id, source, task_type, status, payload, created_by, created_at, completed_at
         FROM admin_tasks ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, source, task_type, status, payload, created_by, created_at, completed_at)| {
            json!({
                "id": id, "source": source, "type": task_type, "status": status,
                "payload": payload, "created_by": created_by, "created_at": created_at,
                "completed_at": completed_at,
            })
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(json!({ "items": items, "total": total, "page": 1, "pageNum": 200 })))
}

/// POST /api/v1/admin/jobs/tasks/{task_id}/complete — mark an admin task done.
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs/tasks/{task_id}/complete",
    responses(
        (status = 200, description = "task completed", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn complete_task(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(task_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "coupon:manage").await?;
    let db = state.require_db()?;
    sqlx::query(
        "UPDATE admin_tasks SET status = 'completed', completed_at = now()
         WHERE id = $1 AND status != 'completed'",
    )
    .bind(task_id)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(Json(json!({ "ok": true, "task_id": task_id })))
}

/// GET /api/v1/admin/jobs — recent jobs.
#[utoipa::path(
    get,
    path = "/api/v1/admin/jobs",
    responses(
        (status = 200, description = "job list", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Job>>, ApiError> {
    state.permissions.require(&state.db, &auth, "dashboard:view").await?;
    let db = state.require_db()?;
    let jobs = sqlx::query_as::<_, Job>(
        "SELECT id, type, state, progress, stage, created_at, started_at, finished_at, error
         FROM jobs ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "jobs query failed");
        ApiError::internal()
    })?;
    Ok(Json(jobs))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateJobBody {
    #[serde(rename = "type")]
    pub job_type: String,
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/admin/jobs — start a new job.
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs",
    request_body = CreateJobBody,
    responses(
        (status = 200, description = "job started", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn create(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateJobBody>,
) -> Result<Json<Value>, ApiError> {
    // Job types map to permissions.
    let permission = match body.job_type.as_str() {
        "pmp.update.check" | "pmp.update.apply" => "server:update",
        "ppf.build" => "server:manage",
        "backup" => "server:manage",
        _ => return Err(ApiError::validation("unknown job type")),
    };
    state.permissions.require(&state.db, &auth, permission).await?;
    let job = state.jobs.start(&body.job_type, body.args).await?;
    Ok(Json(json!({ "job": job })))
}

/// GET /api/v1/admin/jobs/{job_id} — job detail.
#[utoipa::path(
    get,
    path = "/api/v1/admin/jobs/{job_id}",
    responses(
        (status = 200, description = "job detail", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn get_job(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Job>, ApiError> {
    state.permissions.require(&state.db, &auth, "dashboard:view").await?;
    let db = state.require_db()?;
    let job = sqlx::query_as::<_, Job>(
        "SELECT id, type, state, progress, stage, created_at, started_at, finished_at, error
         FROM jobs WHERE id = $1",
    )
    .bind(job_id)
    .fetch_optional(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "job query failed");
        ApiError::internal()
    })?
    .ok_or_else(|| ApiError::not_found("job"))?;
    Ok(Json(job))
}

/// POST /api/v1/admin/jobs/{job_id}/cancel — cancel a running job.
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs/{job_id}/cancel",
    responses(
        (status = 200, description = "cancelled", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn cancel(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "dashboard:view").await?;
    state.jobs.cancel(job_id).await?;
    Ok(Json(json!({ "cancelled": job_id })))
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::not_found("job")
    } else {
        tracing::error!(error = %e, "jobs db error");
        ApiError::internal()
    }
}
