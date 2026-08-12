//! Replay community policy domain — policy ONLY, never Replay content (design §12).
//!
//! PPB stores replay_overrides / replay_acl / replay_share_links. Share links
//! store only a token hash; the raw token is returned once and never persisted.
//! Replay data itself is always pulled from PMP `persist.touches/judges`.

pub mod persist;
pub mod routes;
pub mod visibility;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ReplayOverride {
    pub id: Uuid,
    #[serde(rename = "pmpReplayId")]
    pub pmp_replay_id: String,
    #[serde(rename = "playerPhiraId")]
    pub player_phira_id: i64,
    #[serde(rename = "ownerUserId")]
    pub owner_user_id: Option<Uuid>,
    pub visibility: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct ReplayShareLink {
    pub id: Uuid,
    #[serde(rename = "replayRound")]
    pub replay_round: String,
    #[serde(rename = "playerPhiraId")]
    pub player_phira_id: i64,
    #[serde(rename = "tokenHash")]
    pub token_hash: String,
    #[serde(rename = "createdBy")]
    pub created_by: Uuid,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(rename = "revokedAt")]
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Generate an opaque share token and its SHA-256 hash.
pub fn new_share_token() -> (String, String) {
    let mut bytes = [0u8; 24];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    let token = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
    let hash = hash_token(&token);
    (token, hash)
}

/// SHA-256 hex of a token.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn set_visibility(
    db: &sqlx::PgPool,
    pmp_replay_id: &str,
    player_phira_id: i64,
    owner_user_id: Uuid,
    visibility: &str,
) -> Result<ReplayOverride, ApiError> {
    sqlx::query_as::<_, ReplayOverride>(
        "INSERT INTO replay_overrides (pmp_replay_id, player_phira_id, owner_user_id, visibility)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (pmp_replay_id, player_phira_id) DO UPDATE
            SET visibility = EXCLUDED.visibility, updated_at = now()
         RETURNING id, pmp_replay_id, player_phira_id, owner_user_id, visibility, updated_at",
    )
    .bind(pmp_replay_id)
    .bind(player_phira_id)
    .bind(owner_user_id)
    .bind(visibility)
    .fetch_one(db)
    .await
    .map_err(db_err)
}

pub async fn create_share_link(
    db: &sqlx::PgPool,
    replay_round: &str,
    player_phira_id: i64,
    created_by: Uuid,
    expires_at: Option<DateTime<Utc>>,
) -> Result<(ReplayShareLink, String), ApiError> {
    let (token, hash) = new_share_token();
    let link = sqlx::query_as::<_, ReplayShareLink>(
        "INSERT INTO replay_share_links (replay_round, player_phira_id, token_hash, created_by, expires_at)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, replay_round, player_phira_id, token_hash, created_by, expires_at, revoked_at",
    )
    .bind(replay_round)
    .bind(player_phira_id)
    .bind(&hash)
    .bind(created_by)
    .bind(expires_at)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    Ok((link, token))
}

/// Share-link lookup row: (round, player_phira_id, expires_at, revoked_at).
type ShareLinkRow = (String, i64, Option<DateTime<Utc>>, Option<DateTime<Utc>>);

/// Validate an opaque share token; returns the pinned Replay identity
/// `(round_uuid, player_phira_id)` if valid (S-3).
pub async fn resolve_share_token(
    db: &sqlx::PgPool,
    token: &str,
) -> Result<(String, i64), ApiError> {
    let hash = hash_token(token);
    let row: Option<ShareLinkRow> = sqlx::query_as::<_, ShareLinkRow>(
        "SELECT replay_round, player_phira_id, expires_at, revoked_at
         FROM replay_share_links WHERE token_hash = $1",
    )
    .bind(&hash)
    .fetch_optional(db)
    .await
    .map_err(db_err)?;

    match row {
        Some((round, player, expires_at, revoked_at)) => {
            if revoked_at.is_some() {
                return Err(ApiError::new(ErrorCode::NotFound, "share link revoked"));
            }
            if let Some(exp) = expires_at {
                if Utc::now() > exp {
                    return Err(ApiError::new(ErrorCode::NotFound, "share link expired"));
                }
            }
            Ok((round, player))
        }
        None => Err(ApiError::new(ErrorCode::NotFound, "invalid share token")),
    }
}

pub async fn revoke_share_link(db: &sqlx::PgPool, link_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("UPDATE replay_share_links SET revoked_at = now() WHERE id = $1")
        .bind(link_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "replay db error");
        ApiError::internal()
    }
}

/// Crate-visible error mapper (used by replay routes).
pub(crate) fn db_err_public(e: sqlx::Error) -> ApiError {
    db_err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip_hash() {
        let (token, hash) = new_share_token();
        assert_eq!(hash, hash_token(&token));
        assert_ne!(hash, hash_token(&format!("{token}x")));
    }

    #[test]
    fn token_is_url_safe() {
        let (token, _) = new_share_token();
        assert!(!token.contains('/'));
        assert!(!token.contains('+'));
        assert!(!token.contains('='));
    }
}
