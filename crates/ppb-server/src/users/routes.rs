//! Admin user routes (design §18.4): PPB account + PMP player unified view.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::model::User;
use super::repo as user_repo;
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;

pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{user_id}", get(user_detail))
        .route("/users/{user_id}/ban", post(ban_user))
        .route("/users/{user_id}/unban", post(unban_user))
        .route("/users/{user_id}/kick", post(kick_user))
        .route("/users/{user_id}/ip-history", get(ip_history))
}

#[derive(Debug, Deserialize)]
pub struct UserListParams {
    #[serde(rename = "phiraId")]
    pub phira_id: Option<i64>,
    pub search: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// GET /api/v1/admin/users — search PPB accounts (by phira_id or username).
async fn list_users(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<UserListParams>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:view")
        .await?;
    let db = state.require_db()?;
    let page = params.page.unwrap_or(1).max(1);
    let page_num = params.page_num.unwrap_or(20);
    if !(1..=100).contains(&page_num) {
        return Err(ApiError::validation("pageNum must be between 1 and 100"));
    }
    let offset = (page - 1) * page_num;

    let rows: Vec<User> = if let Some(pid) = params.phira_id {
        sqlx::query_as::<_, User>(
            "SELECT id, phira_id, username_cache, avatar_cache, status, created_at, updated_at, last_seen_at
             FROM users WHERE phira_id = $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(pid)
        .bind(page_num)
        .bind(offset)
        .fetch_all(db)
        .await
        .map_err(db_err)?
    } else if let Some(search) = params.search.as_deref().filter(|s| !s.is_empty()) {
        sqlx::query_as::<_, User>(
            "SELECT id, phira_id, username_cache, avatar_cache, status, created_at, updated_at, last_seen_at
             FROM users WHERE username_cache ILIKE $1 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
        )
        .bind(format!("%{search}%"))
        .bind(page_num)
        .bind(offset)
        .fetch_all(db)
        .await
        .map_err(db_err)?
    } else {
        sqlx::query_as::<_, User>(
            "SELECT id, phira_id, username_cache, avatar_cache, status, created_at, updated_at, last_seen_at
             FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(page_num)
        .bind(offset)
        .fetch_all(db)
        .await
        .map_err(db_err)?
    };

    let total: (i64,) = sqlx::query_as::<_, (i64,)>("SELECT COUNT(*) FROM users")
        .fetch_one(db)
        .await
        .map_err(db_err)?;

    Ok(Json(json!({
        "items": rows,
        "total": total.0,
        "page": page,
        "pageNum": page_num,
    })))
}

/// GET /api/v1/admin/users/{user_id} — PPB account + PMP player info.
async fn user_detail(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:view")
        .await?;
    let db = state.require_db()?;
    let user = user_repo::find_by_phira_id(db, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;

    // Best-effort PMP player info (PMP offline -> null, not fatal).
    let player = state
        .player
        .info(user_id as i32)
        .await
        .map(|v| serde_json::to_value(&v).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);

    let groups = group_ids_for_user(db, user.id).await?;
    Ok(Json(json!({
        "account": user,
        "groups": groups,
        "player": player,
    })))
}

async fn group_ids_for_user(db: &sqlx::PgPool, user_id: uuid::Uuid) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as::<_, (String,)>(
        "SELECT g.name FROM group_members gm JOIN groups g ON g.id = gm.group_id WHERE gm.user_id = $1 ORDER BY g.name",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

#[derive(Debug, Deserialize)]
pub struct BanBody {
    #[serde(default)]
    pub reason: Option<String>,
}

async fn ban_user(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
    Json(body): Json<BanBody>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:ban")
        .await?;
    let result = state
        .player
        .ban(user_id as i32, body.reason.as_deref().unwrap_or("banned via Panel"))
        .await
        .map_err(ApiError::from)?;
    crate::audit::service::record_principal(
        state.require_db()?,
        &auth,
        "user.ban",
        "user",
        &user_id.to_string(),
        json!({"user_id": user_id}),
        "success",
        "",
        "",
        "",
        "",
    )
    .await?;
    Ok(Json(result))
}

async fn unban_user(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:ban")
        .await?;
    let result = state
        .player
        .unban(user_id as i32)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn kick_user(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:kick")
        .await?;
    let result = state
        .player
        .kick(user_id as i32)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

async fn ip_history(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:view_ip_history")
        .await?;
    let result = state
        .player
        .ip_history(user_id as i32)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(result))
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(crate::error::ErrorCode::NotFound, "user not found")
    } else {
        tracing::error!(error = %e, "user db error");
        ApiError::internal()
    }
}
