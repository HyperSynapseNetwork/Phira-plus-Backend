//! Group bootstrap + membership services (design §8.3).
//!
//! Bootstrap: Administrators (admin_scope), Moderators, Developers, Members
//! (default). Non-root users must belong to >= 1 group.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

pub const GROUP_ADMINISTRATORS: &str = "Administrators";
pub const GROUP_MODERATORS: &str = "Moderators";
pub const GROUP_DEVELOPERS: &str = "Developers";
pub const GROUP_MEMBERS: &str = "Members";

#[derive(Debug, Clone, Serialize, FromRow, utoipa::ToSchema)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub system_kind: Option<String>,
    pub is_default: bool,
    pub protected: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Create the four seed groups if missing. Idempotent.
pub async fn bootstrap_groups(db: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    ensure_group(db, GROUP_ADMINISTRATORS, "系统管理员（admin_scope）", Some("admin_scope"), false, true).await?;
    ensure_group(db, GROUP_MODERATORS, "版主模板组", None, false, false).await?;
    ensure_group(db, GROUP_DEVELOPERS, "开发者模板组", None, false, false).await?;
    ensure_group(db, GROUP_MEMBERS, "默认成员组", None, true, true).await?;
    Ok(())
}

async fn ensure_group(
    db: &sqlx::PgPool,
    name: &str,
    description: &str,
    system_kind: Option<&str>,
    is_default: bool,
    protected: bool,
) -> Result<(), sqlx::Error> {
    // Create if absent. Only a NEWLY inserted row may flip is_default; we never
    // reset an existing (possibly admin-changed) default group on bootstrap
    // (Gate 2).
    let result = sqlx::query(
        "INSERT INTO groups (name, description, system_kind, is_default, protected)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (name) DO NOTHING",
    )
    .bind(name)
    .bind(description)
    .bind(system_kind)
    .bind(is_default)
    .bind(protected)
    .execute(db)
    .await?;

    // Only clear other defaults when we actually created a new default-flagged row.
    if result.rows_affected() > 0 && is_default {
        sqlx::query("UPDATE groups SET is_default = (name = $1)")
            .bind(name)
            .execute(db)
            .await?;
    }
    Ok(())
}

/// Return the current default group id (creating Members if none exists).
pub async fn default_group_id(db: &sqlx::PgPool) -> Result<Uuid, ApiError> {
    let row: Option<(Uuid,)> =
        sqlx::query_as::<_, (Uuid,)>("SELECT id FROM groups WHERE is_default = TRUE LIMIT 1")
            .fetch_optional(db)
            .await
            .map_err(db_err)?;
    if let Some((id,)) = row {
        return Ok(id);
    }
    // No default: try to (re)assign Members.
    bootstrap_groups(db).await.map_err(db_err)?;
    let row: (Uuid,) = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM groups WHERE name = $1")
        .bind(GROUP_MEMBERS)
        .fetch_one(db)
        .await
        .map_err(db_err)?;
    Ok(row.0)
}

/// Ensure a user belongs to at least one group (the default group).
pub async fn ensure_user_in_default_group(
    db: &sqlx::PgPool,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let membership: (bool,) =
        sqlx::query_as::<_, (bool,)>("SELECT EXISTS(SELECT 1 FROM group_members WHERE user_id = $1)")
            .bind(user_id)
            .fetch_one(db)
            .await
            .map_err(db_err)?;
    if membership.0 {
        return Ok(());
    }
    let gid = default_group_id(db).await?;
    sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(gid)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// List groups (with member counts).
pub async fn list_groups(db: &sqlx::PgPool) -> Result<Vec<GroupWithCount>, ApiError> {
    let groups = sqlx::query_as::<_, Group>(
        "SELECT id, name, description, system_kind, is_default, protected, created_at, updated_at
         FROM groups ORDER BY name",
    )
    .fetch_all(db)
    .await
    .map_err(db_err)?;

    let mut out = Vec::new();
    for g in groups {
        let (count,): (i64,) = sqlx::query_as::<_, (i64,)>(
            "SELECT COUNT(*) FROM group_members WHERE group_id = $1",
        )
        .bind(g.id)
        .fetch_one(db)
        .await
        .map_err(db_err)?;
        out.push(GroupWithCount { group: g, member_count: count });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupWithCount {
    #[serde(flatten)]
    pub group: Group,
    pub member_count: i64,
}

/// Typed group list item (§22: group fields + member_count + permissions).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GroupListItem {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub system_kind: Option<String>,
    pub is_default: bool,
    pub protected: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub member_count: i64,
    pub permissions: Vec<String>,
}

/// Paginated group list response (§22: `{items, total, page, pageNum}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GroupListResponse {
    pub items: Vec<GroupListItem>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "group not found")
    } else {
        tracing::error!(error = %e, "group db error");
        ApiError::internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_names_stable() {
        assert_eq!(GROUP_MEMBERS, "Members");
        assert_eq!(GROUP_ADMINISTRATORS, "Administrators");
    }
}
