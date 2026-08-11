//! Server-side sessions. Refresh secret is stored as a hash (never plaintext).

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use super::types::{ClientType, PrincipalType};
use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, FromRow)]
pub struct Session {
    pub id: Uuid,
    pub principal_type: String,
    pub user_id: Option<Uuid>,
    pub client_type: String,
    pub refresh_hash: String,
    pub device_name: String,
    pub ip: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl Session {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none() && Utc::now() < self.expires_at
    }
}

/// Generate a new random session refresh token (32 bytes, hex).
pub fn generate_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    hex(bytes.as_slice())
}

/// Hash a refresh token for storage (`refresh_hash`).
pub fn hash_refresh_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex(digest.as_slice())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn create_session(
    db: &sqlx::PgPool,
    principal_type: PrincipalType,
    user_id: Option<Uuid>,
    client_type: ClientType,
    refresh_hash: &str,
    ttl_secs: i64,
    device_name: &str,
    ip: &str,
) -> Result<Session, ApiError> {
    let expires_at = Utc::now() + chrono::Duration::seconds(ttl_secs);
    sqlx::query_as::<_, Session>(
        "INSERT INTO sessions
           (principal_type, user_id, client_type, refresh_hash, device_name, ip, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, principal_type, user_id, client_type, refresh_hash, device_name, ip,
                   created_at, expires_at, revoked_at, last_seen_at",
    )
    .bind(principal_type.to_string())
    .bind(user_id)
    .bind(client_type.to_string())
    .bind(refresh_hash)
    .bind(device_name)
    .bind(ip)
    .bind(expires_at)
    .fetch_one(db)
    .await
    .map_err(db_err)
}

pub async fn find_active_by_refresh_hash(
    db: &sqlx::PgPool,
    refresh_hash: &str,
) -> Result<Option<Session>, ApiError> {
    sqlx::query_as::<_, Session>(
        "SELECT id, principal_type, user_id, client_type, refresh_hash, device_name, ip,
                created_at, expires_at, revoked_at, last_seen_at
         FROM sessions
         WHERE refresh_hash = $1 AND revoked_at IS NULL AND expires_at > now()",
    )
    .bind(refresh_hash)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

pub async fn find_by_id(db: &sqlx::PgPool, id: Uuid) -> Result<Option<Session>, ApiError> {
    sqlx::query_as::<_, Session>(
        "SELECT id, principal_type, user_id, client_type, refresh_hash, device_name, ip,
                created_at, expires_at, revoked_at, last_seen_at
         FROM sessions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

pub async fn revoke(db: &sqlx::PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn revoke_all_for_user(db: &sqlx::PgPool, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE sessions SET revoked_at = now() WHERE user_id = $1 AND revoked_at IS NULL")
        .bind(user_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn touch(db: &sqlx::PgPool, id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE sessions SET last_seen_at = now() WHERE id = $1")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "session not found")
    } else {
        tracing::error!(error = %e, "session db error");
        ApiError::internal()
    }
}
