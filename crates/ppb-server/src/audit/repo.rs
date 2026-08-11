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

/// Filtered audit list (Panel §18.12).
#[allow(clippy::too_many_arguments)]
pub async fn list_filtered(
    db: &sqlx::PgPool,
    action: Option<&str>,
    principal_type: Option<&str>,
    actor: Option<uuid::Uuid>,
    result: Option<&str>,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditEvent>, ApiError> {
    let mut sql = String::from(
        "SELECT id, occurred_at, principal_type, actor_user_id, actor_session_id, action,
                resource_type, resource_id, parameters_redacted, result, error_code,
                request_id, command_id, ip, user_agent
         FROM audit_events WHERE 1=1",
    );
    if action.is_some() {
        sql.push_str(" AND action = $");
        sql.push_str(&(next_placeholder(&sql)).to_string());
    }
    if principal_type.is_some() {
        sql.push_str(" AND principal_type = $");
        sql.push_str(&(next_placeholder(&sql)).to_string());
    }
    if actor.is_some() {
        sql.push_str(" AND actor_user_id = $");
        sql.push_str(&(next_placeholder(&sql)).to_string());
    }
    if result.is_some() {
        sql.push_str(" AND result = $");
        sql.push_str(&(next_placeholder(&sql)).to_string());
    }
    sql.push_str(" ORDER BY occurred_at DESC LIMIT $");
    sql.push_str(&(next_placeholder(&sql)).to_string());
    sql.push_str(" OFFSET $");
    sql.push_str(&(next_placeholder(&sql)).to_string());

    let mut q = sqlx::query_as::<_, AuditEvent>(&sql);
    if let Some(a) = action {
        q = q.bind(a);
    }
    if let Some(p) = principal_type {
        q = q.bind(p);
    }
    if let Some(a) = actor {
        q = q.bind(a);
    }
    if let Some(r) = result {
        q = q.bind(r);
    }
    q = q.bind(limit).bind(offset);
    q.fetch_all(db).await.map_err(db_err)
}

/// Fetch a single audit event by id.
pub async fn get(db: &sqlx::PgPool, id: uuid::Uuid) -> Result<Option<AuditEvent>, ApiError> {
    sqlx::query_as::<_, AuditEvent>(
        "SELECT id, occurred_at, principal_type, actor_user_id, actor_session_id, action,
                resource_type, resource_id, parameters_redacted, result, error_code,
                request_id, command_id, ip, user_agent
         FROM audit_events WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

fn next_placeholder(sql: &str) -> usize {
    sql.matches('$').count() + 1
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "audit event not found")
    } else {
        tracing::error!(error = %e, "audit db error");
        ApiError::internal()
    }
}
