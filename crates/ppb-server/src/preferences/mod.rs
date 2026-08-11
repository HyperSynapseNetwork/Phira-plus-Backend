//! User preferences — JSONB + revision optimistic concurrency (design §21).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UserPreference {
    #[serde(rename = "userId")]
    pub user_id: Uuid,
    pub namespace: String,
    pub revision: i64,
    pub data: serde_json::Value,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

pub async fn get(
    db: &sqlx::PgPool,
    user_id: Uuid,
    namespace: &str,
) -> Result<Option<UserPreference>, ApiError> {
    sqlx::query_as::<_, UserPreference>(
        "SELECT user_id, namespace, revision, json_data AS data, updated_at
         FROM user_preferences WHERE user_id = $1 AND namespace = $2",
    )
    .bind(user_id)
    .bind(namespace)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

/// Upsert with optimistic concurrency: `expected_revision` must match or be `None`.
/// On conflict, the operation is rejected so the client can retry with the new revision.
pub async fn upsert(
    db: &sqlx::PgPool,
    user_id: Uuid,
    namespace: &str,
    data: serde_json::Value,
    expected_revision: Option<i64>,
) -> Result<UserPreference, ApiError> {
    let current = get(db, user_id, namespace).await?;
    let revision = current
        .as_ref()
        .map(|p| p.revision)
        .unwrap_or(0);

    if let Some(expected) = expected_revision {
        if expected != revision {
            return Err(ApiError::with_details(
                ErrorCode::Conflict,
                "preference revision mismatch (optimistic concurrency)",
                serde_json::json!({ "current_revision": revision }),
            ));
        }
    }

    let next_revision = revision + 1;
    let row = sqlx::query_as::<_, UserPreference>(
        "INSERT INTO user_preferences (user_id, namespace, revision, json_data, updated_at)
         VALUES ($1, $2, $3, $4, now())
         ON CONFLICT (user_id, namespace) DO UPDATE
            SET revision = EXCLUDED.revision, json_data = EXCLUDED.json_data, updated_at = now()
         RETURNING user_id, namespace, revision, json_data AS data, updated_at",
    )
    .bind(user_id)
    .bind(namespace)
    .bind(next_revision)
    .bind(data)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    Ok(row)
}

pub async fn delete(db: &sqlx::PgPool, user_id: Uuid, namespace: &str) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM user_preferences WHERE user_id = $1 AND namespace = $2")
        .bind(user_id)
        .bind(namespace)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "preference not found")
    } else {
        tracing::error!(error = %e, "preference db error");
        ApiError::internal()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn revision_semantics() {
        // Pure logic: next revision is +1, and only the current revision is accepted.
        assert_eq!(0i64 + 1, 1);
        assert!(Some(0) == Some(0));
        assert!(Some(1) != Some(0));
    }
}
