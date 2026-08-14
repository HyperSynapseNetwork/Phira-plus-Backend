//! `/api/v1/admin/jobs/*` routes.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::Job;
use crate::app::AppState;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
use crate::error::extractors::{ApiJson, ApiPath, ApiQuery};
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};


#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobRetryResponse {
    pub ok: bool,
    pub job_id: Uuid,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AdminTaskItem {
    pub id: Uuid,
    pub source: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub status: String,
    pub payload: Value,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct AdminTaskListParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AdminTaskListResponse {
    pub items: Vec<AdminTaskItem>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AdminTaskCompleteResponse {
    pub ok: bool,
    pub task_id: Uuid,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CreateJobResponse {
    pub job: Job,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobCancelResponse {
    pub cancelled: Uuid,
}

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
///
/// Re-runs the exact same permission + reauth gate as Create (per the job
/// type's Policy Registry entry) — never a blanket `server:update`.
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs/{job_id}/retry",
    operation_id = "admin_jobs_job_id_retry_post",
    responses(
        (status = 200, description = "job re-queued", body = JobRetryResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
        (status = 409, description = "job not retryable", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn retry_job(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiPath(job_id): ApiPath<Uuid>,
) -> Result<Json<JobRetryResponse>, ApiError> {
    let descriptor = state.jobs.descriptor(job_id).await?;
    state
        .permissions
        .require(&state.db, &auth, descriptor.permission)
        .await?;
    if let Some(risk) = descriptor.reauth {
        check_reauth_header(&state, &auth, &headers, risk)?;
    }
    if !descriptor.retryable {
        return Err(ApiError::new(ErrorCode::JobNotRetryable, "job type is not retryable"));
    }
    state.jobs.retry(job_id).await?;
    Ok(Json(JobRetryResponse { ok: true, job_id }))
}

/// GET /api/v1/admin/jobs/tasks — manual admin tasks (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/admin/jobs/tasks",
    operation_id = "admin_jobs_tasks_get",
    responses(
        (status = 200, description = "admin tasks", body = AdminTaskListResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list_tasks(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<AdminTaskListParams>,
) -> Result<Json<AdminTaskListResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "coupon:view").await?;
    let db = state.require_db()?;
    let page = params.page.unwrap_or(1).max(1);
    let page_num = params.page_num.unwrap_or(50).clamp(1, 200);
    let status = params.status.as_deref().filter(|v| matches!(*v, "pending" | "completed"));
    if params.status.is_some() && status.is_none() {
        return Err(ApiError::new(ErrorCode::ValidationFailed, "status must be pending or completed"));
    }
    let offset = (page - 1) * page_num;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM admin_tasks WHERE ($1::text IS NULL OR status = $1)",
    )
    .bind(status)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Value, Option<Uuid>, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT id, source, task_type, status, payload, created_by, created_at, completed_at
         FROM admin_tasks WHERE ($1::text IS NULL OR status = $1)
         ORDER BY created_at DESC LIMIT $2 OFFSET $3",
    )
    .bind(status)
    .bind(page_num)
    .bind(offset)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let items = rows
        .into_iter()
        .map(|(id, source, task_type, status, payload, created_by, created_at, completed_at)| AdminTaskItem {
            id, source, task_type, status, payload, created_by, created_at, completed_at,
        })
        .collect();
    Ok(Json(AdminTaskListResponse { items, total, page, page_num }))
}

/// POST /api/v1/admin/jobs/tasks/{task_id}/complete — mark an admin task done.
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs/tasks/{task_id}/complete",
    operation_id = "admin_jobs_tasks_task_id_complete_post",
    responses(
        (status = 200, description = "task completed", body = AdminTaskCompleteResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn complete_task(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(task_id): ApiPath<Uuid>,
) -> Result<Json<AdminTaskCompleteResponse>, ApiError> {
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
    Ok(Json(AdminTaskCompleteResponse { ok: true, task_id }))
}

#[derive(Debug, Deserialize)]
pub struct JobListParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// Paginated job list response (§22 `{items, total, page, pageNum}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct JobListResponse {
    pub items: Vec<Job>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

/// GET /api/v1/admin/jobs — recent jobs (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/admin/jobs",
    operation_id = "admin_jobs_get",
    responses(
        (status = 200, description = "job list", body = JobListResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<JobListParams>,
) -> Result<Json<JobListResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "dashboard:view").await?;
    let db = state.require_db()?;
    let page = params.page.unwrap_or(1).max(1);
    let page_num = params.page_num.unwrap_or(50).clamp(1, 100);
    let offset = (page - 1) * page_num;
    let total: (i64,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM jobs")
        .fetch_one(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "jobs count failed");
            ApiError::internal()
        })?;
    let jobs = sqlx::query_as::<_, Job>(
        "SELECT id, type, state, progress, stage, created_at, started_at, finished_at, error
         FROM jobs ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(page_num)
    .bind(offset)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "jobs query failed");
        ApiError::internal()
    })?;
    Ok(Json(JobListResponse {
        items: jobs,
        total: total.0,
        page,
        page_num,
    }))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateJobBody {
    #[serde(rename = "type")]
    pub job_type: String,
}

/// POST /api/v1/admin/jobs — start a new job.
///
/// The Job Policy Registry is the single source of permission / reauth /
/// executor. Clients supply only a job type — never CLI text (`args.command`
/// is no longer accepted).
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs",
    operation_id = "admin_jobs_post",
    request_body = CreateJobBody,
    responses(
        (status = 200, description = "job started", body = CreateJobResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn create(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CreateJobBody>,
) -> Result<Json<CreateJobResponse>, ApiError> {
    let descriptor = state
        .job_registry
        .get(&body.job_type)
        .ok_or_else(|| ApiError::new(ErrorCode::JobTypeUnknown, "unknown job type"))?;
    state
        .permissions
        .require(&state.db, &auth, descriptor.permission)
        .await?;
    if let Some(risk) = descriptor.reauth {
        check_reauth_header(&state, &auth, &headers, risk)?;
    }
    let job = state.jobs.start(&body.job_type).await?;
    Ok(Json(CreateJobResponse { job }))
}

/// GET /api/v1/admin/jobs/{job_id} — job detail.
#[utoipa::path(
    get,
    path = "/api/v1/admin/jobs/{job_id}",
    operation_id = "admin_jobs_job_id_get",
    responses(
        (status = 200, description = "job detail", body = Job),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn get_job(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(job_id): ApiPath<Uuid>,
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
    .ok_or_else(|| ApiError::new(ErrorCode::JobNotFound, "job not found"))?;
    Ok(Json(job))
}

/// POST /api/v1/admin/jobs/{job_id}/cancel — cancel a queued job.
///
/// Permission is checked per job type (not `dashboard:view`). A job that has
/// already been dispatched cannot be cancelled (returns 409).
#[utoipa::path(
    post,
    path = "/api/v1/admin/jobs/{job_id}/cancel",
    operation_id = "admin_jobs_job_id_cancel_post",
    responses(
        (status = 200, description = "cancelled", body = JobCancelResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
        (status = 409, description = "cannot cancel dispatched/finished job", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn cancel(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(job_id): ApiPath<Uuid>,
) -> Result<Json<JobCancelResponse>, ApiError> {
    let descriptor = state.jobs.descriptor(job_id).await?;
    state
        .permissions
        .require(&state.db, &auth, descriptor.permission)
        .await?;
    state.jobs.cancel(job_id).await?;
    Ok(Json(JobCancelResponse { cancelled: job_id }))
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::JobNotFound, "job not found")
    } else {
        tracing::error!(error = %e, "jobs db error");
        ApiError::internal()
    }
}
