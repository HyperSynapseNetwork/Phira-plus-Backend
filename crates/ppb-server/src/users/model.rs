//! PPB user model. `phira_id` is the external stable identity; internal PK is a UUID.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: Uuid,
    #[serde(rename = "phiraId")]
    pub phira_id: i64,
    #[serde(rename = "usernameCache")]
    pub username_cache: String,
    #[serde(rename = "avatarCache")]
    pub avatar_cache: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTime<Utc>,
    #[serde(rename = "lastSeenAt")]
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }
}
