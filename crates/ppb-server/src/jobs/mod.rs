//! Long-running Job domain (design §9.4). Model + repo + runner + routes.

pub mod registry;
pub mod routes;
pub mod runner;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Clone, Serialize, FromRow, utoipa::ToSchema)]
pub struct Job {
    pub id: Uuid,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub r#type: String,
    pub state: String,
    pub progress: Option<f32>,
    pub stage: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: String,
}

pub async fn create(
    db: &sqlx::PgPool,
    job_type: &str,
    resource_key: &str,
) -> Result<Job, ApiError> {
    sqlx::query_as::<_, Job>(
        "INSERT INTO jobs (type, resource_key) VALUES ($1, $2)
         RETURNING id, type, state, progress, stage, created_at, started_at, finished_at, error",
    )
    .bind(job_type)
    .bind(resource_key)
    .fetch_one(db)
    .await
    .map_err(db_err)
}

pub async fn update_state(
    db: &sqlx::PgPool,
    id: Uuid,
    state: &str,
    stage: &str,
    progress: Option<f32>,
    error: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE jobs
         SET state = $2, stage = $3, progress = COALESCE($4, progress),
             error = $5,
             started_at = CASE WHEN $2 = 'running' AND started_at IS NULL THEN now() ELSE started_at END,
             finished_at = CASE WHEN $2 IN ('succeeded','failed','cancelled','not_implemented') THEN now() ELSE finished_at END
         WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(stage)
    .bind(progress)
    .bind(error)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    tracing::error!(error = %e, "job db error");
    ApiError::internal()
}
