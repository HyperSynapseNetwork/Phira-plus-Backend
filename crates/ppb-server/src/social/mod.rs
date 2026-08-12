//! Social domain — bidirectional friends + blocks (design §7.4).
//! Never extends into private chat/IM.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct FriendRequest {
    pub id: Uuid,
    pub from_user_id: Uuid,
    pub to_user_id: Uuid,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub responded_at: Option<DateTime<Utc>>,
}

pub async fn send_request(
    db: &sqlx::PgPool,
    from_user_id: Uuid,
    to_user_id: Uuid,
) -> Result<FriendRequest, ApiError> {
    if from_user_id == to_user_id {
        return Err(ApiError::validation("cannot friend yourself"));
    }
    // Reject if already friends or a request exists.
    let (friend,): (bool,) = sqlx::query_as::<_, (bool,)>(
        "SELECT EXISTS(
            SELECT 1 FROM friendships WHERE (user_a = $1 AND user_b = $2) OR (user_a = $2 AND user_b = $1)
         )",
    )
    .bind(from_user_id)
    .bind(to_user_id)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    if friend {
        return Err(ApiError::new(ErrorCode::Conflict, "already friends"));
    }
    sqlx::query_as::<_, FriendRequest>(
        "INSERT INTO friend_requests (from_user_id, to_user_id, status)
         VALUES ($1, $2, 'pending')
         ON CONFLICT (from_user_id, to_user_id) DO NOTHING
         RETURNING id, from_user_id, to_user_id, status, created_at, responded_at",
    )
    .bind(from_user_id)
    .bind(to_user_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| ApiError::new(ErrorCode::Conflict, "friend request already sent"))
}

pub async fn respond_request(
    db: &sqlx::PgPool,
    request_id: Uuid,
    to_user_id: Uuid,
    accept: bool,
) -> Result<(), ApiError> {
    let request = sqlx::query_as::<_, FriendRequest>(
        "SELECT id, from_user_id, to_user_id, status, created_at, responded_at
         FROM friend_requests WHERE id = $1",
    )
    .bind(request_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| ApiError::not_found("friend request"))?;

    if request.to_user_id != to_user_id {
        return Err(ApiError::permission_denied());
    }
    if request.status != "pending" {
        return Err(ApiError::new(ErrorCode::Conflict, "request already handled"));
    }

    let new_status = if accept { "accepted" } else { "declined" };
    let mut tx = db.begin().await.map_err(db_err)?;
    sqlx::query("UPDATE friend_requests SET status = $2, responded_at = now() WHERE id = $1")
        .bind(request_id)
        .bind(new_status)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    if accept {
        let (a, b) = normalize_pair(request.from_user_id, request.to_user_id);
        sqlx::query("INSERT INTO friendships (user_a, user_b) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(a)
            .bind(b)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(())
}

/// Normalize a friendship pair so (A,B) and (B,A) collapse to one row.
pub fn normalize_pair(a: Uuid, b: Uuid) -> (Uuid, Uuid) {
    if a < b {
        (a, b)
    } else {
        (b, a)
    }
}

pub async fn remove_friend(db: &sqlx::PgPool, a: Uuid, b: Uuid) -> Result<(), ApiError> {
    let (x, y) = normalize_pair(a, b);
    sqlx::query("DELETE FROM friendships WHERE user_a = $1 AND user_b = $2")
        .bind(x)
        .bind(y)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn list_friends(db: &sqlx::PgPool, user_id: Uuid) -> Result<Vec<Uuid>, ApiError> {
    let rows: Vec<(Uuid,)> = sqlx::query_as::<_, (Uuid,)>(
        "SELECT CASE WHEN user_a = $1 THEN user_b ELSE user_a END
         FROM friendships WHERE user_a = $1 OR user_b = $1",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn block(db: &sqlx::PgPool, blocker: Uuid, blocked: Uuid) -> Result<(), ApiError> {
    if blocker == blocked {
        return Err(ApiError::validation("cannot block yourself"));
    }
    sqlx::query("INSERT INTO user_blocks (blocker_id, blocked_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(blocker)
        .bind(blocked)
        .execute(db)
        .await
        .map_err(db_err)?;
    // Removing a block also removes any friendship.
    let _ = remove_friend(db, blocker, blocked).await;
    Ok(())
}

pub async fn unblock(db: &sqlx::PgPool, blocker: Uuid, blocked: Uuid) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2")
        .bind(blocker)
        .bind(blocked)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "social db error");
        ApiError::internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_normalization() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let (x1, y1) = normalize_pair(a, b);
        let (x2, y2) = normalize_pair(b, a);
        assert_eq!((x1, y1), (x2, y2));
    }
}
