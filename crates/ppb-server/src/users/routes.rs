//! Admin user routes (design §18.4): PPB account + PMP player unified view.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::oneshot;
use uuid::Uuid;

use super::model::{
    GroupRef, SessionItem, User, UserDetailResponse, UserListResponse, UserMultiplayerResponse,
    UserSecurityResponse, UserSessionsResponse,
};
use super::repo as user_repo;
use crate::actions::types::Risk;
use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
use crate::commands::broker::{redact_args, CommandAudit, CommandTask};
use crate::commands::repo as command_repo;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn admin_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{phira_id}", get(user_detail))
        .route("/users/{phira_id}/multiplayer", get(user_multiplayer))
        .route("/users/{phira_id}/sessions", get(user_sessions))
        .route("/users/{phira_id}/security", get(user_security))
        .route("/users/{phira_id}/audit", get(user_audit))
        .route("/users/{phira_id}/actions", post(user_actions))
        .route("/users/{phira_id}/ban", post(ban_user))
        .route("/users/{phira_id}/unban", post(unban_user))
        .route("/users/{phira_id}/kick", post(kick_user))
        .route("/users/{phira_id}/ip-history", get(ip_history))
}

#[derive(Debug, Deserialize)]
pub struct UserListParams {
    pub phira_id: Option<i64>,
    pub search: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// GET /api/v1/admin/users — search PPB accounts (by phira_id or username).
#[utoipa::path(
    get,
    path = "/api/v1/admin/users",
    operation_id = "admin_users_get",
    responses(
        (status = 200, description = "paginated users", body = UserListResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list_users(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<UserListParams>,
) -> Result<Json<UserListResponse>, ApiError> {
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

    let items = rows.iter().map(User::to_admin_item).collect();
    Ok(Json(UserListResponse {
        items,
        total: total.0,
        page,
        page_num,
    }))
}

/// GET /api/v1/admin/users/{phira_id} — PPB account + PMP player info (§22
/// `{account, groups, player}`; path is Phira ID).
#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{phira_id}",
    operation_id = "admin_users_phira_id_get",
    responses(
        (status = 200, description = "user detail", body = UserDetailResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn user_detail(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<UserDetailResponse>, ApiError> {
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
        .ok();

    let groups = groups_for_user(db, user.id).await?;
    Ok(Json(UserDetailResponse {
        account: user.to_admin_item(),
        groups,
        player,
    }))
}

async fn groups_for_user(db: &sqlx::PgPool, user_id: uuid::Uuid) -> Result<Vec<GroupRef>, ApiError> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT g.id, g.name FROM group_members gm JOIN groups g ON g.id = gm.group_id WHERE gm.user_id = $1 ORDER BY g.name",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().map(|(id, name)| GroupRef { id, name }).collect())
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
    headers: HeaderMap,
    Json(body): Json<BanBody>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:ban")
        .await?;
    // §23 #10 Sensitive Action Policy: user ban always requires reauth.
    check_reauth_header(&state, &auth, &headers, ReauthRisk::High)?;
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
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:ban")
        .await?;
    // §23 #10 Sensitive Action Policy: user unban always requires reauth.
    check_reauth_header(&state, &auth, &headers, ReauthRisk::High)?;
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

// ── §17 user subpaths ───────────────────────────────────────────

/// GET /api/v1/admin/users/{id}/multiplayer — PMP player + presence (best-effort).
#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{phira_id}/multiplayer",
    operation_id = "admin_users_user_id_multiplayer_get",
    responses(
        (status = 200, description = "player + presence", body = UserMultiplayerResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn user_multiplayer(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<UserMultiplayerResponse>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:view")
        .await?;
    let player = state.player.info(user_id as i32).await.ok();
    let online = player
        .as_ref()
        .and_then(|p| p.get("online").and_then(Value::as_bool))
        .unwrap_or(false);
    let current_room = player
        .as_ref()
        .and_then(|p| p.get("room_id").and_then(Value::as_str))
        .map(str::to_string);
    let ban_state = player
        .as_ref()
        .and_then(|p| p.get("banned").and_then(Value::as_bool))
        .unwrap_or(false);
    Ok(Json(UserMultiplayerResponse {
        phira_id: user_id,
        online,
        current_room,
        ban_state,
        // PMP does not expose these — null rather than fabricated.
        playtime_secs: None,
        rounds_played: None,
        replay_count: None,
    }))
}

/// GET /api/v1/admin/users/{id}/sessions — PPB web/desktop sessions.
#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{phira_id}/sessions",
    operation_id = "admin_users_user_id_sessions_get",
    responses(
        (status = 200, description = "user sessions", body = UserSessionsResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn user_sessions(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<UserSessionsResponse>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:view")
        .await?;
    let db = state.require_db()?;
    let user = user_repo::find_by_phira_id(db, user_id)
        .await?
        .ok_or_else(|| ApiError::not_found("user"))?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, chrono::DateTime<chrono::Utc>, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT id, client_type, device_name, ip, created_at, revoked_at
         FROM sessions WHERE user_id = $1 ORDER BY created_at DESC LIMIT 100",
    )
    .bind(user.id)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let items: Vec<SessionItem> = rows
        .into_iter()
        .map(|(id, client_type, device_name, ip, created_at, revoked_at)| SessionItem {
            id,
            client_type,
            device_name,
            ip,
            created_at,
            revoked_at,
        })
        .collect();
    Ok(Json(UserSessionsResponse { items }))
}

/// GET /api/v1/admin/users/{id}/security — ban/IP-ban state (best-effort).
#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{phira_id}/security",
    operation_id = "admin_users_user_id_security_get",
    responses(
        (status = 200, description = "user security state", body = UserSecurityResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn user_security(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<UserSecurityResponse>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "user:view")
        .await?;
    let player = state.player.info(user_id as i32).await.ok();
    let ban_state = player
        .as_ref()
        .and_then(|p| p.get("banned").and_then(Value::as_bool))
        .unwrap_or(false);

    // Ban reason from the PMP banlist (BanEntry {user_id, reason}).
    let ban_reason = state
        .player
        .banlist()
        .await
        .ok()
        .and_then(|v| v.get("bans").and_then(Value::as_array).cloned())
        .and_then(|bans| {
            bans.iter()
                .find(|b| b.get("user_id").and_then(Value::as_i64) == Some(user_id))
                .and_then(|b| b.get("reason").and_then(Value::as_str).map(str::to_string))
        });

    // IP history from PMP (dynamic list; empty when unavailable).
    let ip_history = state
        .player
        .ip_history(user_id as i32)
        .await
        .ok()
        .and_then(|v| v.get("ip_history").and_then(Value::as_array).cloned())
        .unwrap_or_default();

    Ok(Json(UserSecurityResponse {
        phira_id: user_id,
        ban_state,
        ban_reason,
        ip_history,
        // PMP exposes no IP-ban list / banned_at over OpenUDS — null.
        ip_bans: None,
        banned_at: None,
    }))
}

/// GET /api/v1/admin/users/{id}/audit — audit events targeting this user.
#[utoipa::path(
    get,
    path = "/api/v1/admin/users/{phira_id}/audit",
    operation_id = "admin_users_user_id_audit_get",
    responses(
        (status = 200, description = "user audit events", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn user_audit(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "audit:view")
        .await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, crate::audit::model::AuditEvent>(
        "SELECT id, occurred_at, principal_type, actor_user_id, actor_session_id, action,
                resource_type, resource_id, parameters_redacted, result, error_code,
                request_id, command_id, ip, user_agent
         FROM audit_events
         WHERE (actor_user_id = (SELECT id FROM users WHERE phira_id = $1))
            OR (resource_type = 'user' AND resource_id = $1::text)
         ORDER BY occurred_at DESC LIMIT 100",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(Json(json!({ "items": rows })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UserActionBody {
    pub action: String,
    #[serde(default)]
    pub args: Value,
}

/// POST /api/v1/admin/users/{id}/actions — run a registered action scoped to a user.
#[utoipa::path(
    post,
    path = "/api/v1/admin/users/{phira_id}/actions",
    operation_id = "admin_users_user_id_actions_post",
    request_body = UserActionBody,
    responses(
        (status = 200, description = "action result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]

/// POST /api/v1/admin/users/{id}/actions — run a registered action scoped to a
/// user (e.g. player.kick / player.ban). The phira_id is injected as user_id.
pub async fn user_actions(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(user_id): Path<i64>,
    Json(body): Json<UserActionBody>,
) -> Result<axum::response::Response, ApiError> {
    let action = state
        .actions
        .get(&body.action)
        .ok_or_else(|| ApiError::not_found("action"))?;
    let mut args = body.args;
    if args.get("user_id").is_none() {
        if let Value::Object(map) = &mut args {
            map.insert("user_id".to_string(), json!(user_id));
        }
    }
    if !state
        .permissions
        .has_permission(&state.db, &auth, action.permission)
        .await?
    {
        return Err(ApiError::permission_denied());
    }
    if action.reauth {
        let risk = if action.risk >= Risk::Critical {
            ReauthRisk::Critical
        } else {
            ReauthRisk::High
        };
        check_reauth_header(&state, &auth, &headers, risk)?;
    }

    let db = state.require_db()?;
    let queue_key = state.actions.resolve_queue_key(action, &args);
    let command_id = Uuid::new_v4();
    let args_redacted = redact_args(&args);
    command_repo::insert_queued(db, command_id, action.id, &auth.sub.to_string(), &queue_key, args_redacted.clone())
        .await?;
    // Gate 0 A5: audited actions are recorded by the executor with the FINAL
    // result once the command completes — no pre-recorded success.
    let audit = if action.audit {
        Some(CommandAudit {
            principal_type: auth.principal_type.to_string(),
            actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
            actor_session_id: auth.sid,
            action: action.id.to_string(),
            resource_type: "user".to_string(),
            resource_id: user_id.to_string(),
            request_id: auth.request_id.clone(),
            ip: ip_from_headers(&headers),
            user_agent: user_agent_from_headers(&headers),
        })
    } else {
        None
    };

    let (completion, rx) = if action.long_running {
        (None, None)
    } else {
        let (tx, rx) = oneshot::channel();
        (Some(tx), Some(rx))
    };
    state
        .commands
        .submit(CommandTask {
            command_id,
            action: action.id.to_string(),
            actor: auth.sub.to_string(),
            resource_key: queue_key,
            args,
            args_redacted,
            completion,
            audit,
        })?;
    if let Some(rx) = rx {
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(Ok(v))) => Ok(Json(v).into_response()),
            Ok(Ok(Err(e))) => Err(ApiError::new(ErrorCode::PmpUnavailable, e)),
            Ok(Err(_)) => Err(ApiError::new(ErrorCode::PmpUnavailable, "executor dropped")),
            Err(_) => Err(ApiError::new(ErrorCode::PmpUnavailable, "command timed out")),
        }
    } else {
        let accepted = json!({ "command_id": command_id, "status": "queued" });
        Ok((axum::http::StatusCode::ACCEPTED, Json(accepted)).into_response())
    }
}

fn ip_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(str::trim)
        .unwrap_or("")
        .to_string()
}

fn user_agent_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(crate::error::ErrorCode::NotFound, "user not found")
    } else {
        tracing::error!(error = %e, "user db error");
        ApiError::internal()
    }
}
