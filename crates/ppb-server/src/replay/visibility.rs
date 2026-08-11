//! Replay visibility enforcement (design §12.3, contract §10/§13).
//!
//! Default visibility is Public. `replay_overrides` set per-round visibility:
//! inherit | public | friends | private | unlisted | custom. Custom uses
//! `replay_acl` allow/deny. Share links are explicit grants (token-hash only).

use uuid::Uuid;

use crate::auth::types::AuthPrincipal;
use crate::error::{ApiError, ErrorCode};

use super::{resolve_share_token, ReplayOverride};

pub const VISIBILITY_PUBLIC: &str = "public";
pub const VISIBILITY_FRIENDS: &str = "friends";
pub const VISIBILITY_PRIVATE: &str = "private";
pub const VISIBILITY_UNLISTED: &str = "unlisted";
pub const VISIBILITY_CUSTOM: &str = "custom";
pub const VISIBILITY_INHERIT: &str = "inherit";

/// Fetch the effective visibility for a round (defaults to public).
pub async fn effective_visibility(
    db: &sqlx::PgPool,
    round_uuid: &str,
) -> Result<String, ApiError> {
    let row: Option<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT visibility FROM replay_overrides WHERE pmp_replay_id = $1",
    )
    .bind(round_uuid)
    .fetch_optional(db)
    .await
    .map_err(db_err)?;
    match row {
        Some((v,)) if v != VISIBILITY_INHERIT => Ok(v),
        _ => Ok(VISIBILITY_PUBLIC.to_string()),
    }
}

/// True if `requester` may access `round_uuid`. `share_token` (opaque link)
/// is an explicit grant and bypasses visibility. Unauthenticated callers may
/// only access public rounds (or via a valid share token).
pub async fn check_replay_access(
    db: &sqlx::PgPool,
    round_uuid: &str,
    requester: Option<&AuthPrincipal>,
    share_token: Option<&str>,
) -> Result<bool, ApiError> {
    if let Some(token) = share_token {
        // Throws NotFound when invalid/expired/revoked.
        resolve_share_token(db, token).await?;
        return Ok(true);
    }
    let visibility = effective_visibility(db, round_uuid).await?;
    if visibility == VISIBILITY_PUBLIC {
        return Ok(true);
    }
    let Some(auth) = requester else {
        return Ok(false); // non-public requires authentication
    };
    if auth.is_root() {
        return Ok(true);
    }

    let override_row = sqlx::query_as::<_, ReplayOverride>(
        "SELECT id, pmp_replay_id, owner_user_id, visibility, updated_at
         FROM replay_overrides WHERE pmp_replay_id = $1",
    )
    .bind(round_uuid)
    .fetch_optional(db)
    .await
    .map_err(db_err)?;

    let Some(over) = override_row else {
        // No override row but non-public visibility (shouldn't happen): deny.
        return Ok(false);
    };
    let owner = over.owner_user_id.unwrap_or_default();
    if auth.sub == owner {
        return Ok(true); // owner always
    }

    match visibility.as_str() {
        VISIBILITY_FRIENDS => {
            let friends = crate::social::list_friends(db, owner).await?;
            Ok(friends.contains(&auth.sub))
        }
        VISIBILITY_UNLISTED => Ok(true), // known uuid is the grant
        VISIBILITY_PRIVATE => Ok(false),
        VISIBILITY_CUSTOM => {
            // allow/deny ACL; deny wins.
            let acl = sqlx::query_as::<_, (String,)>( // result: (effect,)
                "SELECT effect FROM replay_acl WHERE replay_id = $1 AND user_id = $2",
            )
            .bind(over.id)
            .bind(auth.sub)
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
            match acl {
                Some((effect,)) if effect == "allow" => Ok(true),
                Some((effect,)) if effect == "deny" => Ok(false),
                _ => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "replay visibility db error");
        ApiError::internal()
    }
}
