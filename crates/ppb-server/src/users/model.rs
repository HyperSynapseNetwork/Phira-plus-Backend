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

/// A user's group membership reference (§23 `{id,name}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GroupRef {
    pub id: Uuid,
    pub name: String,
}

/// User detail response (§22/§23 `{account, groups:[{id,name}], player}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserDetailResponse {
    pub account: AdminUserItem,
    pub groups: Vec<GroupRef>,
    /// Best-effort PMP player info (dynamic payload; null when PMP offline).
    pub player: Option<serde_json::Value>,
}

/// User multiplayer subview (§23 #5: presence/current_room/ban_state typed;
/// playtime/rounds/replay counts null when PMP doesn't provide them).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserMultiplayerResponse {
    pub phira_id: i64,
    pub online: bool,
    pub current_room: Option<String>,
    pub ban_state: bool,
    pub playtime_secs: Option<i64>,
    pub rounds_played: Option<i64>,
    pub replay_count: Option<i64>,
}

/// One session row (§23 #5).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SessionItem {
    pub id: Uuid,
    pub client_type: String,
    pub device_name: String,
    pub ip: String,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// User sessions subview (§23 #5).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserSessionsResponse {
    pub items: Vec<SessionItem>,
}

/// User security subview (§23 #5).
///
/// P-87 carve-out: `ip_history` is PMP `player.ip_history` payload (PMP is the
/// multiplayer truth source, §13) and stays dynamic JSON rather than a PPB
/// reverse-engineered schema. `ip_bans` / `banned_at` are always `null` — PMP
/// exposes no IP-ban list or ban timestamp over OpenUDS, so PPB returns null
/// rather than fabricating a value.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct UserSecurityResponse {
    pub phira_id: i64,
    pub ban_state: bool,
    pub ban_reason: Option<String>,
    pub ip_history: Vec<serde_json::Value>,
    pub ip_bans: Option<serde_json::Value>,
    pub banned_at: Option<serde_json::Value>,
}
