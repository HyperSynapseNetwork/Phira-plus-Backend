//! Group management repo (admin operations).

use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

use super::groups::Group;
use super::manifest::PermissionDef;
use super::resolver::PermissionResolver;
use crate::error::{ApiError, ErrorCode};

pub async fn create_group(
    db: &sqlx::PgPool,
    name: &str,
    description: &str,
    is_default: Option<bool>,
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
                return ApiError::new(ErrorCode::ResourceConflict, "group name already exists");
            }
        }
        db_err(e)
    })?;

    // §23 #4 semantics: only one default group globally. Creating a new default
    // clears the old default (mirrors set_default_group / PATCH).
    if is_default == Some(true) {
        sqlx::query("UPDATE groups SET is_default = (id = $1)")
            .bind(group.id)
            .execute(db)
            .await
            .map_err(db_err)?;
        return get_group(db, group.id).await;
    }
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

/// Replace a group's full member set (contract §17 Groups `PUT /members`).
/// Users removed from their last group are re-added to the default group
/// (Gate 2: a user must belong to at least one group).
pub async fn replace_group_members(db: &sqlx::PgPool, group_id: Uuid, user_ids: &[Uuid]) -> Result<(), ApiError> {
    let existing = list_group_members(db, group_id).await?;
    sqlx::query("DELETE FROM group_members WHERE group_id = $1")
        .bind(group_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    for user_id in user_ids {
        sqlx::query("INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(group_id)
            .bind(user_id)
            .execute(db)
            .await
            .map_err(db_err)?;
    }
    let new_set: std::collections::HashSet<Uuid> = user_ids.iter().cloned().collect();
    for m in existing {
        if !new_set.contains(&m.user_id) {
            ensure_user_not_orphaned(db, m.user_id).await?;
        }
    }
    Ok(())
}

/// Replace a group's full permission set (contract §17 Groups `PUT /permissions`).
/// Rejects `*:*` and unknown permission ids at the API layer.
pub async fn replace_group_permissions(
    db: &sqlx::PgPool,
    group_id: Uuid,
    permissions: &[String],
) -> Result<(), ApiError> {
    for p in permissions {
        PermissionResolver::validate_group_permission(p)?;
        if PermissionResolver::new().permission_by_id(p).is_none() {
            return Err(ApiError::validation(format!("unknown permission id: {p}")));
        }
    }
    sqlx::query("DELETE FROM group_permissions WHERE group_id = $1")
        .bind(group_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    for p in permissions {
        sqlx::query("INSERT INTO group_permissions (group_id, permission) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(group_id)
            .bind(p)
            .execute(db)
            .await
            .map_err(db_err)?;
    }
    Ok(())
}

pub async fn remove_member(db: &sqlx::PgPool, group_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group_id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    // A user must stay in at least one group (Gate 2).
    ensure_user_not_orphaned(db, user_id).await
}

/// If `user_id` now has zero group memberships, add them to the default group.
async fn ensure_user_not_orphaned(db: &sqlx::PgPool, user_id: Uuid) -> Result<(), ApiError> {
    let (count,): (i64,) = sqlx::query_as::<_, (i64,)>(
        "SELECT COUNT(*) FROM group_members WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_one(db)
    .await
    .map_err(db_err)?;
    if count == 0 {
        let default_id = super::groups::default_group_id(db).await?;
        sqlx::query(
            "INSERT INTO group_members (group_id, user_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(default_id)
        .bind(user_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    }
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

/// Switch the default group to `group_id` (clears others). An `admin_scope`
/// group can never be the default (Gate 2).
pub async fn set_default_group(db: &sqlx::PgPool, group_id: Uuid) -> Result<(), ApiError> {
    let group = get_group(db, group_id).await?;
    if group.system_kind.as_deref() == Some("admin_scope") {
        return Err(ApiError::validation(
            "admin_scope group cannot be the default group",
        ));
    }
    sqlx::query("UPDATE groups SET is_default = (id = $1)")
        .bind(group_id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// §23 #4: patch a group atomically — optional `name`/`description`/`is_default`
/// in a single transaction (is_default carries set-default semantics).
pub async fn patch_group(
    db: &sqlx::PgPool,
    group_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
    is_default: Option<bool>,
) -> Result<Group, ApiError> {
    let group = get_group(db, group_id).await?;
    let mut tx = db.begin().await.map_err(db_err)?;

    let new_name = name.map(|s| s.trim()).filter(|s| !s.is_empty());
    if let Some(n) = new_name {
        sqlx::query("UPDATE groups SET name = $1, updated_at = now() WHERE id = $2")
            .bind(n)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    if let Some(d) = description {
        sqlx::query("UPDATE groups SET description = $1, updated_at = now() WHERE id = $2")
            .bind(d)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
    }
    if let Some(default) = is_default {
        if default {
            if group.system_kind.as_deref() == Some("admin_scope") {
                return Err(ApiError::validation(
                    "admin_scope group cannot be the default group",
                ));
            }
            sqlx::query("UPDATE groups SET is_default = (id = $1)")
                .bind(group_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
        }
        // is_default=false is a no-op (the default is only switched by setting
        // another group default); clearing is not supported.
    }
    tx.commit().await.map_err(db_err)?;
    get_group(db, group_id).await
}

/// A group member with the PPB account display fields.
#[derive(Debug, Clone, Serialize, FromRow, utoipa::ToSchema)]
pub struct GroupMember {
    pub user_id: Uuid,
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
            ErrorCode::ResourceConflict,
            format!("protected group '{}' cannot be deleted", group.name),
        ));
    }
    if group.is_default {
        return Err(ApiError::new(
            ErrorCode::ResourceConflict,
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
        ApiError::new(ErrorCode::ResourceNotFound, "not found")
    } else {
        tracing::error!(error = %e, "group db error");
        ApiError::internal()
    }
}
