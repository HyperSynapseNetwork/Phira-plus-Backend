//! Users repository.

use uuid::Uuid;

use super::model::User;
use crate::error::{ApiError, ErrorCode};

pub async fn find_by_phira_id(db: &sqlx::PgPool, phira_id: i64) -> Result<Option<User>, ApiError> {
    sqlx::query_as::<_, User>(
        "SELECT id, phira_id, username_cache, avatar_cache, status, created_at, updated_at, last_seen_at
         FROM users WHERE phira_id = $1",
    )
    .bind(phira_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

pub async fn find_by_id(db: &sqlx::PgPool, id: Uuid) -> Result<Option<User>, ApiError> {
    sqlx::query_as::<_, User>(
        "SELECT id, phira_id, username_cache, avatar_cache, status, created_at, updated_at, last_seen_at
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

/// Upsert a user by phira_id; returns the row.
pub async fn upsert_by_phira_id(
    db: &sqlx::PgPool,
    phira_id: i64,
    username: &str,
    avatar: &str,
) -> Result<User, ApiError> {
    sqlx::query_as::<_, User>(
        "INSERT INTO users (phira_id, username_cache, avatar_cache, last_seen_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (phira_id) DO UPDATE
            SET username_cache = EXCLUDED.username_cache,
                avatar_cache = EXCLUDED.avatar_cache,
                last_seen_at = now(),
                updated_at = now()
         RETURNING id, phira_id, username_cache, avatar_cache, status, created_at, updated_at, last_seen_at",
    )
    .bind(phira_id)
    .bind(username)
    .bind(avatar)
    .fetch_one(db)
    .await
    .map_err(db_err)
}

pub async fn touch_last_seen(db: &sqlx::PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE users SET last_seen_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::UserNotFound, "user not found")
    } else {
        tracing::error!(error = %e, "user db error");
        ApiError::internal()
    }
}
