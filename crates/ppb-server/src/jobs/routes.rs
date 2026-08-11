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
use crate::error::ApiError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/jobs", get(list).post(create))
        .route("/jobs/{job_id}", get(get_job))
        .route("/jobs/{job_id}/cancel", post(cancel))
}

async fn list(
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

#[derive(Debug, Deserialize)]
pub struct CreateJobBody {
    #[serde(rename = "type")]
    pub job_type: String,
    #[serde(default)]
    pub args: Value,
}

async fn create(
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

async fn get_job(
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

async fn cancel(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "dashboard:view").await?;
    state.jobs.cancel(job_id).await?;
    Ok(Json(json!({ "cancelled": job_id })))
}
