//! Notification Hub (design §14). Event/inbox separated; push endpoints encrypted.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct NotificationEvent {
    pub id: Uuid,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(rename = "actorUserId")]
    pub actor_user_id: Option<Uuid>,
    pub payload: serde_json::Value,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UserNotification {
    pub id: Uuid,
    #[serde(rename = "eventId")]
    pub event_id: Uuid,
    #[serde(rename = "userId")]
    pub user_id: Uuid,
    #[serde(rename = "readAt")]
    pub read_at: Option<DateTime<Utc>>,
    #[serde(rename = "dismissedAt")]
    pub dismissed_at: Option<DateTime<Utc>>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
}

/// Wire schema (contract §8): `{type, priority, title, body, actor, target, actions, input, deep_link, expires_at, dedup_key}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationPayload {
    #[serde(rename = "type")]
    pub notification_type: String,
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub actor: Option<i64>,
    #[serde(default)]
    pub target: serde_json::Value,
    #[serde(default)]
    pub actions: Vec<serde_json::Value>,
    #[serde(default)]
    pub input: Option<serde_json::Value>,
    #[serde(rename = "deep_link", default)]
    pub deep_link: String,
    #[serde(rename = "expires_at")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(rename = "dedup_key", default)]
    pub dedup_key: String,
}

pub async fn create_event(
    db: &sqlx::PgPool,
    notification_type: &str,
    actor_user_id: Option<Uuid>,
    payload: serde_json::Value,
) -> Result<NotificationEvent, ApiError> {
    sqlx::query_as::<_, NotificationEvent>(
        "INSERT INTO notification_events (type, actor_user_id, payload)
         VALUES ($1, $2, $3)
         RETURNING id, type, actor_user_id, payload, created_at",
    )
    .bind(notification_type)
    .bind(actor_user_id)
    .bind(payload)
    .fetch_one(db)
    .await
    .map_err(db_err)
}

/// Create an event and fan out inbox rows to a set of users (single transaction).
pub async fn publish_to_users(
    db: &sqlx::PgPool,
    notification_type: &str,
    actor_user_id: Option<Uuid>,
    payload: serde_json::Value,
    recipients: &[Uuid],
) -> Result<NotificationEvent, ApiError> {
    let event = create_event(db, notification_type, actor_user_id, payload).await?;
    let mut tx = db.begin().await.map_err(db_err)?;
    for uid in recipients {
        sqlx::query(
            "INSERT INTO user_notifications (event_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(event.id)
        .bind(uid)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }
    tx.commit().await.map_err(db_err)?;
    Ok(event)
}

/// An inbox row joined with its event payload.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UserNotificationWithEvent {
    pub id: Uuid,
    #[serde(rename = "eventId")]
    pub event_id: Uuid,
    #[serde(rename = "userId")]
    pub user_id: Uuid,
    #[serde(rename = "readAt")]
    pub read_at: Option<DateTime<Utc>>,
    #[serde(rename = "dismissedAt")]
    pub dismissed_at: Option<DateTime<Utc>>,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "eventType")]
    pub event_type: String,
    #[serde(rename = "actorUserId")]
    pub actor_user_id: Option<Uuid>,
    pub payload: serde_json::Value,
    #[serde(rename = "eventCreatedAt")]
    pub event_created_at: DateTime<Utc>,
}

pub async fn list_for_user(
    db: &sqlx::PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<UserNotificationWithEvent>, ApiError> {
    let rows: Vec<UserNotificationWithEvent> = sqlx::query_as(
        "SELECT un.id, un.event_id, un.user_id, un.read_at, un.dismissed_at, un.created_at,
                ev.type AS event_type, ev.actor_user_id, ev.payload, ev.created_at AS event_created_at
         FROM user_notifications un
         JOIN notification_events ev ON ev.id = un.event_id
         WHERE un.user_id = $1
         ORDER BY un.created_at DESC
         LIMIT $2",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows)
}

pub async fn mark_read(db: &sqlx::PgPool, notification_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE user_notifications SET read_at = now()
         WHERE id = $1 AND user_id = $2",
    )
    .bind(notification_id)
    .bind(user_id)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

pub async fn register_push_endpoint(
    db: &sqlx::PgPool,
    user_id: Uuid,
    device_id: &str,
    channel: &str,
    endpoint_ciphertext: &[u8],
    platform: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO push_endpoints (user_id, device_id, channel, endpoint_ciphertext, platform)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (user_id, device_id) DO UPDATE
            SET endpoint_ciphertext = EXCLUDED.endpoint_ciphertext, platform = EXCLUDED.platform,
                last_seen_at = now(), disabled_at = NULL",
    )
    .bind(user_id)
    .bind(device_id)
    .bind(channel)
    .bind(endpoint_ciphertext)
    .bind(platform)
    .execute(db)
    .await
    .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "notification not found")
    } else {
        tracing::error!(error = %e, "notification db error");
        ApiError::internal()
    }
}
