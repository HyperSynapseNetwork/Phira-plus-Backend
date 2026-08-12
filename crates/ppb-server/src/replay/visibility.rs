//! Replay visibility enforcement (design §12.3, contract §20 S-3).
//!
//! A Replay's identity is the pair `(round_uuid, player_phira_id)`. Visibility
//! overrides, ACLs, and share links all bind to the pair. `resolve_replay_access`
//! returns the server-pinned player so the viewer WS can never be redirected to
//! another player's touches/judges.

use crate::auth::types::AuthPrincipal;
use crate::error::{ApiError, ErrorCode};

use super::{resolve_share_token, ReplayOverride};

pub const VISIBILITY_PUBLIC: &str = "public";
pub const VISIBILITY_FRIENDS: &str = "friends";
pub const VISIBILITY_PRIVATE: &str = "private";
pub const VISIBILITY_UNLISTED: &str = "unlisted";
pub const VISIBILITY_CUSTOM: &str = "custom";
pub const VISIBILITY_INHERIT: &str = "inherit";

/// Fetch the effective visibility for a `(round, player)` replay (default public).
pub async fn effective_visibility(
    db: &sqlx::PgPool,
    round_uuid: &str,
    player_phira_id: i64,
) -> Result<String, ApiError> {
    let row: Option<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT visibility FROM replay_overrides
         WHERE pmp_replay_id = $1 AND player_phira_id = $2",
    )
    .bind(round_uuid)
    .bind(player_phira_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?;
    match row {
        Some((v,)) if v != VISIBILITY_INHERIT => Ok(v),
        _ => Ok(VISIBILITY_PUBLIC.to_string()),
    }
}

async fn get_override(
    db: &sqlx::PgPool,
    round_uuid: &str,
    player_phira_id: i64,
) -> Result<Option<ReplayOverride>, ApiError> {
    sqlx::query_as::<_, ReplayOverride>(
        "SELECT id, pmp_replay_id, player_phira_id, owner_user_id, visibility, updated_at
         FROM replay_overrides WHERE pmp_replay_id = $1 AND player_phira_id = $2",
    )
    .bind(round_uuid)
    .bind(player_phira_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)
}

async fn requester_phira_id(
    db: &sqlx::PgPool,
    auth: &AuthPrincipal,
) -> Result<Option<i64>, ApiError> {
    if auth.is_root() {
        return Ok(None);
    }
    let user = crate::users::repo::find_by_id(db, auth.sub).await?;
    Ok(user.map(|u| u.phira_id))
}

/// Check whether `requester` may access `(round_uuid, player_phira_id)`.
pub async fn check_pair_access(
    db: &sqlx::PgPool,
    round_uuid: &str,
    player_phira_id: i64,
    requester: Option<&AuthPrincipal>,
) -> Result<bool, ApiError> {
    let visibility = effective_visibility(db, round_uuid, player_phira_id).await?;
    if visibility == VISIBILITY_PUBLIC {
        return Ok(true);
    }
    let Some(auth) = requester else {
        return Ok(false); // non-public requires authentication
    };
    if auth.is_root() {
        return Ok(true);
    }
    // Owner of the override, or the player themselves (own replay).
    let over = get_override(db, round_uuid, player_phira_id).await?;
    if let Some(o) = &over {
        if o.owner_user_id == Some(auth.sub) {
            return Ok(true);
        }
    }
    if requester_phira_id(db, auth).await? == Some(player_phira_id) {
        return Ok(true);
    }
    let owner = over.as_ref().and_then(|o| o.owner_user_id).unwrap_or_default();
    match visibility.as_str() {
        VISIBILITY_FRIENDS => {
            let friends = crate::social::list_friends(db, owner).await?;
            Ok(friends.contains(&auth.sub))
        }
        // Unlisted replays never appear in lists and are only reachable by
        // non-owners via a valid share token (resolved before this check in
        // `resolve_replay_access`). The owner/player themself already returned
        // above; everyone else is denied here.
        VISIBILITY_UNLISTED => Ok(false),
        VISIBILITY_PRIVATE => Ok(false),
        VISIBILITY_CUSTOM => {
            let Some(over) = over else { return Ok(false) };
            let acl = sqlx::query_as::<_, (String,)>(
                "SELECT effect FROM replay_acl WHERE replay_id = $1 AND user_id = $2",
            )
            .bind(over.id)
            .bind(auth.sub)
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
            match acl {
                Some((effect,)) if effect == "allow" => Ok(true),
                _ => Ok(false),
            }
        }
        _ => Ok(false),
    }
}

/// Resolve the pinned player for a Replay request (S-3).
///
/// - A valid share token pins its own `(round_uuid, player_phira_id)`; it cannot
///   be used to access a different round or player.
/// - Without a token, the requested `player_phira_id` is validated against the
///   requester's access; the returned player is what the caller must use.
/// - `Ok(None)` means access denied.
pub async fn resolve_replay_access(
    db: &sqlx::PgPool,
    round_uuid: &str,
    requested_player: i64,
    requester: Option<&AuthPrincipal>,
    share_token: Option<&str>,
) -> Result<Option<i64>, ApiError> {
    if let Some(token) = share_token {
        let (round, player) = resolve_share_token(db, token).await?;
        if round != round_uuid {
            return Ok(None); // token is bound to a single Replay
        }
        return Ok(Some(player));
    }
    if check_pair_access(db, round_uuid, requested_player, requester).await? {
        Ok(Some(requested_player))
    } else {
        Ok(None)
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
