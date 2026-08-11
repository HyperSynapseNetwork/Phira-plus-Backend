//! Config persistence (overrides, public content, PPF build config, snapshots).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ConfigSnapshot {
    pub id: Uuid,
    pub scope: String,
    pub content: String,
    pub note: String,
    #[serde(rename = "createdBy")]
    pub created_by: Option<Uuid>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "restoredAt")]
    pub restored_at: Option<DateTime<Utc>>,
}

// ── PPB runtime overrides ───────────────────────────────────────

pub async fn get_overrides(db: &sqlx::PgPool) -> Result<Option<serde_json::Value>, ApiError> {
    let row: Option<(serde_json::Value,)> =
        sqlx::query_as::<_, (serde_json::Value,)>("SELECT content FROM ppb_runtime_overrides WHERE id = 1")
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
    Ok(row.map(|r| r.0))
}

pub async fn put_overrides(
    db: &sqlx::PgPool,
    content: serde_json::Value,
    actor: Option<Uuid>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO ppb_runtime_overrides (id, content, updated_by) VALUES (1, $1, $2)
         ON CONFLICT (id) DO UPDATE SET content = EXCLUDED.content, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(content)
    .bind(actor)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

// ── Public content ──────────────────────────────────────────────

pub async fn get_public_content(db: &sqlx::PgPool, key: &str) -> Result<Option<serde_json::Value>, ApiError> {
    let row: Option<(serde_json::Value,)> =
        sqlx::query_as::<_, (serde_json::Value,)>("SELECT content FROM public_content WHERE key = $1")
            .bind(key)
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
    Ok(row.map(|r| r.0))
}

pub async fn put_public_content(
    db: &sqlx::PgPool,
    key: &str,
    content: serde_json::Value,
    actor: Option<Uuid>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO public_content (key, content, updated_by) VALUES ($1, $2, $3)
         ON CONFLICT (key) DO UPDATE SET content = EXCLUDED.content, updated_by = EXCLUDED.updated_by, updated_at = now()",
    )
    .bind(key)
    .bind(content)
    .bind(actor)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

// ── PPF build/SEO config ────────────────────────────────────────

pub async fn get_ppf_config(db: &sqlx::PgPool) -> Result<Option<(i64, serde_json::Value)>, ApiError> {
    let row: Option<(i64, serde_json::Value)> =
        sqlx::query_as::<_, (i64, serde_json::Value)>("SELECT revision, content FROM ppf_build_config WHERE id = 1")
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
    Ok(row)
}

pub async fn put_ppf_config(
    db: &sqlx::PgPool,
    content: serde_json::Value,
    actor: Option<Uuid>,
) -> Result<i64, ApiError> {
    let row: (i64,) = sqlx::query_as::<_, (i64,)>(
        "INSERT INTO ppf_build_config (id, revision, content, updated_by) VALUES (1, 1, $1, $2)
         ON CONFLICT (id) DO UPDATE SET revision = ppf_build_config.revision + 1, content = EXCLUDED.content,
             updated_by = EXCLUDED.updated_by, updated_at = now()
         RETURNING revision",
    )
    .bind(content)
    .bind(actor)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    Ok(row.0)
}

// ── Snapshots ───────────────────────────────────────────────────

pub async fn insert_snapshot(
    db: &sqlx::PgPool,
    scope: &str,
    content: &str,
    note: &str,
    actor: Option<Uuid>,
) -> Result<Uuid, ApiError> {
    let row: (Uuid,) = sqlx::query_as::<_, (Uuid,)>(
        "INSERT INTO config_snapshots (scope, content, note, created_by) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(scope)
    .bind(content)
    .bind(note)
    .bind(actor)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    Ok(row.0)
}

pub async fn list_snapshots(db: &sqlx::PgPool, scope: &str, limit: i64) -> Result<Vec<ConfigSnapshot>, ApiError> {
    sqlx::query_as::<_, ConfigSnapshot>(
        "SELECT id, scope, content, note, created_by, created_at, restored_at
         FROM config_snapshots WHERE scope = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(scope)
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(db_err)
}

pub async fn get_snapshot(db: &sqlx::PgPool, id: Uuid) -> Result<Option<ConfigSnapshot>, ApiError> {
    sqlx::query_as::<_, ConfigSnapshot>(
        "SELECT id, scope, content, note, created_by, created_at, restored_at
         FROM config_snapshots WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

pub async fn mark_restored(db: &sqlx::PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE config_snapshots SET restored_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "config db error");
        ApiError::internal()
    }
}
