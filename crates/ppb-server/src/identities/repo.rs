//! Identity bindings + phira_credentials repository.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::model::{PhiraCredentialState, UserIdentity};
use crate::error::{ApiError, ErrorCode};

// ── user_identities ───────────────────────────────────────────

pub async fn find_by_provider(
    db: &sqlx::PgPool,
    provider: &str,
    provider_id: &str,
) -> Result<Option<UserIdentity>, ApiError> {
    sqlx::query_as::<_, UserIdentity>(
        "SELECT id, user_id, provider, provider_id, provider_name, linked_at
         FROM user_identities WHERE provider = $1 AND provider_id = $2",
    )
    .bind(provider)
    .bind(provider_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

pub async fn list_for_user(db: &sqlx::PgPool, user_id: Uuid) -> Result<Vec<UserIdentity>, ApiError> {
    sqlx::query_as::<_, UserIdentity>(
        "SELECT id, user_id, provider, provider_id, provider_name, linked_at
         FROM user_identities WHERE user_id = $1 ORDER BY linked_at",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(db_err)
}

/// Ensure a phira identity row exists (root identity).
pub async fn upsert_phira_identity(
    db: &sqlx::PgPool,
    user_id: Uuid,
    phira_id: i64,
    username: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO user_identities (user_id, provider, provider_id, provider_name)
         VALUES ($1, 'phira', $2, $3)
         ON CONFLICT (provider, provider_id) DO UPDATE SET provider_name = EXCLUDED.provider_name",
    )
    .bind(user_id)
    .bind(phira_id.to_string())
    .bind(username)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Bind a GitHub identity to an existing PPB user. Never creates bare accounts.
pub async fn bind_github(
    db: &sqlx::PgPool,
    user_id: Uuid,
    github_id: &str,
    github_username: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO user_identities (user_id, provider, provider_id, provider_name)
         VALUES ($1, 'github', $2, $3)
         ON CONFLICT (provider, provider_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(github_id)
    .bind(github_username)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn unbind_github(db: &sqlx::PgPool, user_id: Uuid, github_id: &str) -> Result<(), ApiError> {
    sqlx::query(
        "DELETE FROM user_identities WHERE user_id = $1 AND provider = 'github' AND provider_id = $2",
    )
    .bind(user_id)
    .bind(github_id)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

// ── phira_credentials ──────────────────────────────────────────

/// Store the encrypted refresh token.
pub async fn store_phira_credential(
    db: &sqlx::PgPool,
    user_id: Uuid,
    refresh_token_ciphertext: &[u8],
    refresh_expires_at: DateTime<Utc>,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO phira_credentials (user_id, refresh_token_ciphertext, refresh_expires_at, state)
         VALUES ($1, $2, $3, 'active')
         ON CONFLICT (user_id) DO UPDATE
            SET refresh_token_ciphertext = EXCLUDED.refresh_token_ciphertext,
                refresh_expires_at = EXCLUDED.refresh_expires_at,
                state = 'active',
                updated_at = now()",
    )
    .bind(user_id)
    .bind(refresh_token_ciphertext)
    .bind(refresh_expires_at)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Load encrypted refresh token + expiry for a user.
pub async fn load_phira_credential(
    db: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<Option<(Vec<u8>, DateTime<Utc>, String)>, ApiError> {
    let row: Option<(Vec<u8>, DateTime<Utc>, String)> = sqlx::query_as(
        "SELECT refresh_token_ciphertext, refresh_expires_at, state
         FROM phira_credentials WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?;
    Ok(row)
}

pub async fn mark_reauth_required(db: &sqlx::PgPool, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE phira_credentials SET state = 'reauth_required', updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn revoke_phira_credential(db: &sqlx::PgPool, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE phira_credentials SET state = 'revoked', updated_at = now() WHERE user_id = $1",
    )
    .bind(user_id)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Summarized credential state for /me (never exposes secrets).
pub async fn credential_state(db: &sqlx::PgPool, user_id: Uuid) -> Result<PhiraCredentialState, ApiError> {
    let row: Option<(DateTime<Utc>, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT refresh_expires_at, state, updated_at FROM phira_credentials WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?;

    match row {
        Some((refresh_expires_at, state, updated_at)) => Ok(PhiraCredentialState {
            user_id,
            has_credential: true,
            state,
            refresh_expires_at: Some(refresh_expires_at),
            updated_at: Some(updated_at),
        }),
        None => Ok(PhiraCredentialState {
            user_id,
            has_credential: false,
            state: "none".to_string(),
            refresh_expires_at: None,
            updated_at: None,
        }),
    }
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "identity db error");
        ApiError::internal()
    }
}
