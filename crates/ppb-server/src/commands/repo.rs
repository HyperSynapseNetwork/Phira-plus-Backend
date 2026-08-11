//! command_runs persistence.

use serde_json::Value;
use uuid::Uuid;

use super::model::CommandRun;
use crate::error::{ApiError, ErrorCode};

pub async fn insert_queued(
    db: &sqlx::PgPool,
    id: Uuid,
    action: &str,
    actor: &str,
    resource_key: &str,
    args_redacted: Value,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO command_runs (id, action, actor, resource_key, arguments_redacted)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(id)
    .bind(action)
    .bind(actor)
    .bind(resource_key)
    .bind(args_redacted)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn mark_running(db: &sqlx::PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE command_runs SET status = 'running', started_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn mark_finished(
    db: &sqlx::PgPool,
    id: Uuid,
    status: &str,
    result_summary: &str,
    error_code: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE command_runs
         SET status = $2, finished_at = now(), result_summary = $3, error_code = $4
         WHERE id = $1",
    )
    .bind(id)
    .bind(status)
    .bind(result_summary)
    .bind(error_code)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn list_recent(db: &sqlx::PgPool, limit: i64) -> Result<Vec<CommandRun>, ApiError> {
    sqlx::query_as::<_, CommandRun>(
        "SELECT id, action, actor, resource_key, arguments_redacted, status, started_at,
                finished_at, result_summary, error_code
         FROM command_runs ORDER BY started_at DESC NULLS LAST LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(db_err)
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "command run not found")
    } else {
        tracing::error!(error = %e, "command_runs db error");
        ApiError::internal()
    }
}
