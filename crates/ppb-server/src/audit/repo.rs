//! Audit persistence.

use super::model::{AuditEvent, NewAuditEvent};
use crate::error::{ApiError, ErrorCode};

pub async fn record(db: &sqlx::PgPool, event: &NewAuditEvent) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO audit_events
           (principal_type, actor_user_id, actor_session_id, action, resource_type, resource_id,
            parameters_redacted, result, error_code, request_id, command_id, ip, user_agent)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
    )
    .bind(&event.principal_type)
    .bind(event.actor_user_id)
    .bind(event.actor_session_id)
    .bind(&event.action)
    .bind(&event.resource_type)
    .bind(&event.resource_id)
    .bind(&event.parameters_redacted)
    .bind(&event.result)
    .bind(&event.error_code)
    .bind(&event.request_id)
    .bind(&event.command_id)
    .bind(&event.ip)
    .bind(&event.user_agent)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn list(db: &sqlx::PgPool, limit: i64) -> Result<Vec<AuditEvent>, ApiError> {
    sqlx::query_as::<_, AuditEvent>(
        "SELECT id, occurred_at, principal_type, actor_user_id, actor_session_id, action,
                resource_type, resource_id, parameters_redacted, result, error_code,
                request_id, command_id, ip, user_agent
         FROM audit_events ORDER BY occurred_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(db_err)
}

/// Purge events older than `retention_days`; returns rows deleted.
pub async fn purge_older_than(db: &sqlx::PgPool, retention_days: i32) -> Result<u64, ApiError> {
    let result = sqlx::query(
        "DELETE FROM audit_events WHERE occurred_at < now() - make_interval(days => $1)",
    )
    .bind(retention_days)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(result.rows_affected())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "audit event not found")
    } else {
        tracing::error!(error = %e, "audit db error");
        ApiError::internal()
    }
}
