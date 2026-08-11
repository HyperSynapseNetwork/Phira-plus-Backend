//! Admin routes for the Permission Manifest and groups.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use uuid::Uuid;

use super::groups::{bootstrap_groups, list_groups};
use super::manifest::PermissionDef;
use super::repo;
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/permissions/manifest", get(manifest))
        .route("/groups", get(list).post(create))
        .route("/groups/{id}", patch(rename).delete(delete_group))
        .route("/groups/{id}/permissions", post(add_permission))
        .route(
            "/groups/{id}/permissions/{permission}",
            delete(remove_permission),
        )
        .route("/groups/{id}/members", post(add_member))
        .route("/groups/{id}/members/{user_id}", delete(remove_member))
}

/// GET /api/v1/admin/permissions/manifest
async fn manifest(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PermissionDef>>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:view")
        .await?;
    Ok(Json(state.permissions.manifest().to_vec()))
}

async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:view")
        .await?;
    let db = state.require_db()?;
    let groups = list_groups(db).await?;
    Ok(Json(serde_json::to_value(groups).unwrap_or(serde_json::Value::Null)))
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

async fn create(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateGroupBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:create")
        .await?;
    let db = state.require_db()?;
    let group = repo::create_group(db, &body.name, &body.description).await?;
    Ok(Json(serde_json::to_value(&group).unwrap_or(serde_json::Value::Null)))
}

#[derive(Debug, Deserialize)]
pub struct RenameGroupBody {
    pub name: String,
}

async fn rename(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Json(body): Json<RenameGroupBody>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:edit")
        .await?;
    let db = state.require_db()?;
    repo::rename_group(db, group_id, &body.name).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn delete_group(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:delete")
        .await?;
    let db = state.require_db()?;
    repo::delete_group(db, group_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct PermissionBody {
    pub permission: String,
}

async fn add_permission(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Json(body): Json<PermissionBody>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:edit")
        .await?;
    let db = state.require_db()?;
    repo::add_permission(db, group_id, &body.permission).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn remove_permission(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path((group_id, permission)): Path<(Uuid, String)>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:edit")
        .await?;
    let db = state.require_db()?;
    repo::remove_permission(db, group_id, &permission).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct MemberBody {
    pub user_id: Uuid,
}

async fn add_member(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Json(body): Json<MemberBody>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:assign_user")
        .await?;
    let db = state.require_db()?;
    repo::add_member(db, group_id, body.user_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

async fn remove_member(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path((group_id, user_id)): Path<(Uuid, Uuid)>,
) -> Result<axum::http::StatusCode, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "group:assign_user")
        .await?;
    let db = state.require_db()?;
    repo::remove_member(db, group_id, user_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Ensure bootstrap groups exist at startup.
pub async fn run_bootstrap(db: &sqlx::PgPool) -> Result<(), ApiError> {
    bootstrap_groups(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "group bootstrap failed");
            ApiError::internal()
        })
}
