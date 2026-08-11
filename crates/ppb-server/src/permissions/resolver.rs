//! Permission Resolver — User → Groups → Permissions (V1 path).
//!
//! No direct per-user override. `admin_scope` groups auto-map all
//! `root_only=false` permissions. Root is short-circuited to `*:*`.

use std::collections::HashSet;

use sqlx::FromRow;
use uuid::Uuid;

use super::manifest::{permission_manifest, PermissionDef, ROOT_WILDCARD};
use crate::auth::types::AuthPrincipal;
use crate::error::{ApiError, ErrorCode};

/// A user's group membership joined with its permissions (may be NULLs).
#[derive(Debug, FromRow)]
struct UserGroupPermRow {
    system_kind: Option<String>,
    permission: Option<String>,
}

/// Stateless resolver over the static manifest + DB group graph.
#[derive(Debug, Clone, Default)]
pub struct PermissionResolver {
    _private: (),
}

impl PermissionResolver {
    pub fn new() -> Self {
        Self { _private: () }
    }

    pub fn manifest(&self) -> &'static [PermissionDef] {
        permission_manifest()
    }

    pub fn permission_by_id(&self, id: &str) -> Option<&'static PermissionDef> {
        permission_manifest().iter().find(|p| p.id == id)
    }

    /// Effective permission set for a user principal.
    pub async fn permissions_for_user(
        &self,
        db: &sqlx::PgPool,
        user_id: Uuid,
    ) -> Result<HashSet<String>, sqlx::Error> {
        let rows = sqlx::query_as::<_, UserGroupPermRow>(
            "SELECT g.system_kind, gp.permission
             FROM group_members gm
             JOIN groups g ON g.id = gm.group_id
             LEFT JOIN group_permissions gp ON gp.group_id = g.id
             WHERE gm.user_id = $1",
        )
        .bind(user_id)
        .fetch_all(db)
        .await?;

        let mut perms = HashSet::new();
        let mut admin_scope = false;
        for row in rows {
            if row.system_kind.as_deref() == Some("admin_scope") {
                admin_scope = true;
            }
            if let Some(p) = row.permission {
                // DB CHECK already blocks '*:*'; double-check defensively.
                if p != ROOT_WILDCARD {
                    perms.insert(p);
                }
            }
        }
        if admin_scope {
            for id in PermissionDef::non_root_only_ids() {
                perms.insert(id.to_string());
            }
        }
        Ok(perms)
    }

    /// Effective permission set for a principal (Root → `*:*`).
    pub async fn permissions_for_principal(
        &self,
        db: &Option<sqlx::PgPool>,
        auth: &AuthPrincipal,
    ) -> Result<HashSet<String>, ApiError> {
        if auth.is_root() {
            let mut set = HashSet::new();
            set.insert(ROOT_WILDCARD.to_string());
            return Ok(set);
        }
        let pool = db
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "database not configured"))?;
        self.permissions_for_user(pool, auth.sub)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "permission resolution failed");
                ApiError::internal()
            })
    }

    /// True if the principal holds `permission` (or `*:*`).
    pub async fn has_permission(
        &self,
        db: &Option<sqlx::PgPool>,
        auth: &AuthPrincipal,
        permission: &str,
    ) -> Result<bool, ApiError> {
        let perms = self.permissions_for_principal(db, auth).await?;
        Ok(perms.contains(ROOT_WILDCARD) || perms.contains(permission))
    }

    /// Require a permission, else `PERMISSION_DENIED`.
    pub async fn require(
        &self,
        db: &Option<sqlx::PgPool>,
        auth: &AuthPrincipal,
        permission: &str,
    ) -> Result<(), ApiError> {
        if self.has_permission(db, auth, permission).await? {
            Ok(())
        } else {
            Err(ApiError::permission_denied())
        }
    }

    /// Reject `*:*` for any group (API-layer guard; DB has a CHECK too).
    pub fn validate_group_permission(permission: &str) -> Result<(), ApiError> {
        if permission == ROOT_WILDCARD {
            return Err(ApiError::new(
                ErrorCode::Validation,
                "*:* 仅允许授予 Root，普通用户组禁止",
            ));
        }
        if permission.split(':').count() != 2 {
            return Err(ApiError::validation(
                "permission must be <resource>:<action>",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_wildcard_for_group() {
        assert!(PermissionResolver::validate_group_permission("*:*").is_err());
        assert!(PermissionResolver::validate_group_permission("room:kick").is_ok());
        assert!(PermissionResolver::validate_group_permission("malformed").is_err());
    }

    #[tokio::test]
    async fn root_has_wildcard_without_db() {
        let resolver = PermissionResolver::new();
        let auth = crate::middleware::auth::principal_for_test(
            Uuid::new_v4(),
            Uuid::new_v4(),
            crate::auth::types::PrincipalType::Root,
            crate::auth::types::ClientType::Panel,
        );
        let perms = resolver
            .permissions_for_principal(&None, &auth)
            .await
            .unwrap();
        assert!(perms.contains("*:*"));
        assert!(resolver.has_permission(&None, &auth, "room:kick").await.unwrap());
    }

    #[tokio::test]
    async fn user_without_db_is_internal_error() {
        let resolver = PermissionResolver::new();
        let auth = crate::middleware::auth::principal_for_test(
            Uuid::new_v4(),
            Uuid::new_v4(),
            crate::auth::types::PrincipalType::User,
            crate::auth::types::ClientType::Ppf,
        );
        assert!(resolver.permissions_for_principal(&None, &auth).await.is_err());
    }
}
