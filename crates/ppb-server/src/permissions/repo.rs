//! Group management repo (admin operations).

use serde::Serialize;
use uuid::Uuid;

use super::groups::Group;
use super::manifest::PermissionDef;
use super::resolver::PermissionResolver;
use crate::error::{ApiError, ErrorCode};

pub async fn create_group(
    db: &sqlx::PgPool,
    name: &str,
    description: &str,
) -> Result<Group, ApiError> {
    if name.trim().is_empty() {
        return Err(ApiError::validation("group name required"));
    }
    let group = sqlx::query_as::<_, Group>(
        "INSERT INTO groups (name, description)
         VALUES ($1, $2)
         RETURNING id, name, description, system_kind, is_default, protected, created_at, updated_at",
    )
    .bind(name.trim())
    .bind(description)
    .fetch_one(db)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(dbe) = &e {
            if dbe.is_unique_violation() {
                return ApiError::new(ErrorCode::Conflict, "group name already exists");
            }
        }
        db_err(e)
    })?;
    Ok(group)
}

pub async fn rename_group(
    db: &sqlx::PgPool,
    group_id: Uuid,
    new_name: &str,
) -> Result<(), ApiError> {
    if new_name.trim().is_empty() {
        return Err(ApiError::validation("group name required"));
    }
    sqlx::query("UPDATE groups SET name = $1, updated_at = now() WHERE id = $2")
        .bind(new_name.trim())
        .bind(group_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Add a permission to a group. Rejects `*:*` and unknown permissions.
pub async fn add_permission(
    db: &sqlx::PgPool,
    group_id: Uuid,
    permission: &str,
) -> Result<(), ApiError> {
    PermissionResolver::validate_group_permission(permission)?;
    if PermissionResolver::new().permission_by_id(permission).is_none() {
        return Err(ApiError::validation("unknown permission id"));
    }
    sqlx::query("INSERT INTO group_permissions (group_id, permission) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(group_id)
        .bind(permission)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn remove_permission(
    db: &sqlx::PgPool,
    group_id: Uuid,
    permission: &str,
) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM group_permissions WHERE group_id = $1 AND permission = $2")
        .bind(group_id)
        .bind(permission)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn add_member(db: &sqlx::PgPool, group_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(group_id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

pub async fn remove_member(db: &sqlx::PgPool, group_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group_id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Fetch a single group (or not-found).
pub async fn get_group(db: &sqlx::PgPool, group_id: Uuid) -> Result<Group, ApiError> {
    sqlx::query_as::<_, Group>(
        "SELECT id, name, description, system_kind, is_default, protected, created_at, updated_at
         FROM groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| ApiError::not_found("group"))
}

/// Switch the default group to `group_id` (clears others).
pub async fn set_default_group(db: &sqlx::PgPool, group_id: Uuid) -> Result<(), ApiError> {
    let exists: Option<(Uuid,)> = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(db)
        .await
        .map_err(db_err)?;
    if exists.is_none() {
        return Err(ApiError::not_found("group"));
    }
    sqlx::query("UPDATE groups SET is_default = (id = $1)")
        .bind(group_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// A group member with the PPB account display fields.
#[derive(Debug, Clone, Serialize)]
pub struct GroupMember {
    pub user_id: Uuid,
    #[serde(rename = "phiraId")]
    pub phira_id: i64,
    pub username: String,
}

/// List a group's members (joined with the users table).
pub async fn list_group_members(db: &sqlx::PgPool, group_id: Uuid) -> Result<Vec<GroupMember>, ApiError> {
    sqlx::query_as::<_, GroupMember>(
        "SELECT u.id AS user_id, u.phira_id, u.username_cache AS username
         FROM group_members gm JOIN users u ON u.id = gm.user_id
         WHERE gm.group_id = $1 ORDER BY u.username_cache",
    )
    .bind(group_id)
    .fetch_all(db)
    .await
    .map_err(db_err)
}

/// List a group's explicit permissions.
pub async fn list_group_permissions(db: &sqlx::PgPool, group_id: Uuid) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT permission FROM group_permissions WHERE group_id = $1 ORDER BY permission",
    )
    .bind(group_id)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Effective permission set for a group: `admin_scope` auto-maps all
/// `root_only=false` permissions; ordinary groups use their explicit set.
pub async fn effective_group_permissions(db: &sqlx::PgPool, group_id: Uuid) -> Result<Vec<String>, ApiError> {
    let group = get_group(db, group_id).await?;
    if group.system_kind.as_deref() == Some("admin_scope") {
        let mut ids: Vec<String> = PermissionDef::non_root_only_ids().iter().map(|s| s.to_string()).collect();
        ids.sort();
        Ok(ids)
    } else {
        list_group_permissions(db, group_id).await
    }
}

/// Delete a group. Protected groups and the current default group cannot be deleted.
pub async fn delete_group(db: &sqlx::PgPool, group_id: Uuid) -> Result<(), ApiError> {
    let group = sqlx::query_as::<_, Group>(
        "SELECT id, name, description, system_kind, is_default, protected, created_at, updated_at
         FROM groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_optional(db)
    .await
    .map_err(db_err)?
    .ok_or_else(|| ApiError::not_found("group"))?;

    if group.protected {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            format!("protected group '{}' cannot be deleted", group.name),
        ));
    }
    if group.is_default {
        return Err(ApiError::new(
            ErrorCode::Conflict,
            "current default group cannot be deleted; switch default first",
        ));
    }
    sqlx::query("DELETE FROM groups WHERE id = $1")
        .bind(group_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "group db error");
        ApiError::internal()
    }
}
