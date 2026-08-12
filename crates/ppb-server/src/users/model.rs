//! PPB user model. `phira_id` is the external stable identity; internal PK is a UUID.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct User {
    pub id: Uuid,
    pub phira_id: i64,
    pub username_cache: String,
    pub avatar_cache: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

impl User {
    pub fn is_active(&self) -> bool {
        self.status == "active"
    }

    /// Wire shape (§22): `ppb_user_id` (UUID) + `phira_id` + `username`/`avatar`.
    pub fn to_admin_item(&self) -> AdminUserItem {
        AdminUserItem {
            ppb_user_id: self.id,
            phira_id: self.phira_id,
            username: self.username_cache.clone(),
            avatar: self.avatar_cache.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            last_seen_at: self.last_seen_at,
        }
    }
}

/// Admin user list/detail wire item (§22: `ppb_user_id` UUID, `phira_id`,
/// `username`/`avatar` naming aligned with Panel).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct AdminUserItem {
    pub ppb_user_id: Uuid,
    pub phira_id: i64,
    pub username: String,
    pub avatar: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
}

/// Paginated user list response (§22 `{items, total, page, pageNum}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserListResponse {
    pub items: Vec<AdminUserItem>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

/// User detail response (§22 `{account, groups, player}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserDetailResponse {
    pub account: AdminUserItem,
    pub groups: Vec<String>,
    /// Best-effort PMP player info (dynamic payload; null when PMP offline).
    pub player: Option<serde_json::Value>,
}
