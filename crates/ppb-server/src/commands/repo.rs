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

/// Insert a raw console command run (design §18.10). Unlike typed actions, a
/// console run carries the raw `command` text and a `scope` discriminator.
pub async fn insert_console_run(
    db: &sqlx::PgPool,
    id: Uuid,
    actor: &str,
    command: &str,
    scope: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO command_runs (id, action, actor, command, scope, resource_key)
         VALUES ($1, 'pmp.cli.execute', $2, $3, $4, 'server')",
    )
    .bind(id)
    .bind(actor)
    .bind(command)
    .bind(scope)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Paginated command history. `scope` filters by the stored discriminator
/// (`personal` | `server`); `None` returns all runs.
pub async fn list_recent(
    db: &sqlx::PgPool,
    scope: Option<&str>,
    page: i64,
    page_num: i64,
) -> Result<(Vec<CommandRun>, i64), ApiError> {
    let offset = (page - 1) * page_num;
    let scope_filter: Option<&str> = match scope {
        Some("personal") | Some("server") => scope,
        _ => None,
    };

    let (total,): (i64,) = match scope_filter {
        Some(s) => {
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM command_runs WHERE scope = $1")
                .bind(s)
                .fetch_one(db)
                .await
                .map_err(db_err)?
        }
        None => {
            sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM command_runs")
                .fetch_one(db)
                .await
                .map_err(db_err)?
        }
    };

    let rows = match scope_filter {
        Some(s) => {
            sqlx::query_as::<_, CommandRun>(
                "SELECT id AS command_id, command, action, status,
                        NULLIF(result_summary, '') AS output, NULLIF(error_code, '') AS error,
                        finished_at AS executed_at, actor AS principal, scope
                 FROM command_runs
                 WHERE scope = $1
                 ORDER BY finished_at DESC NULLS LAST
                 LIMIT $2 OFFSET $3",
            )
            .bind(s)
            .bind(page_num)
            .bind(offset)
            .fetch_all(db)
            .await
            .map_err(db_err)?
        }
        None => {
            sqlx::query_as::<_, CommandRun>(
                "SELECT id AS command_id, command, action, status,
                        NULLIF(result_summary, '') AS output, NULLIF(error_code, '') AS error,
                        finished_at AS executed_at, actor AS principal, scope
                 FROM command_runs
                 ORDER BY finished_at DESC NULLS LAST
                 LIMIT $1 OFFSET $2",
            )
            .bind(page_num)
            .bind(offset)
            .fetch_all(db)
            .await
            .map_err(db_err)?
        }
    };

    Ok((rows, total))
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "command run not found")
    } else {
        tracing::error!(error = %e, "command_runs db error");
        ApiError::internal()
    }
}
