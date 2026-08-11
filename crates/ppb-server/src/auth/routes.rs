//! Auth + Root HTTP routes.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::header;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::jwt::{self, ROOT_SUB};
use super::reauth::{self, ReauthClaims, ReauthRisk};
use super::root::RootAuthService;
use super::session::{self, Session};
use super::types::{AuthPrincipal, ClientType, PrincipalType};
use super::{phira as phira_service, ACCESS_COOKIE, REFRESH_COOKIE};
use crate::app::AppState;
use crate::error::{ApiError, ErrorCode};
use crate::identities::repo as identities_repo;
use crate::middleware::cookies::{self, CookieOpts};
use crate::middleware::rate_limit::RateLimiter;
use crate::users::model::User;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phira/login", post(phira_login))
        .route("/phira/reauth", post(phira_reauth))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/github/start", get(github_start))
        .route("/github/callback", get(github_callback))
        .route("/github/unbind", post(github_unbind))
}

pub fn root_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/root/login", post(root_login))
        .route("/auth/root/password", post(root_change_password))
}

// ── Request / response bodies ─────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PhiraLoginRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub return_to: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: Uuid,
    #[serde(rename = "phiraId")]
    pub phira_id: i64,
    pub username: String,
    pub avatar: String,
}

#[derive(Debug, Deserialize)]
pub struct ReauthRequest {
    #[serde(default)]
    pub email: Option<String>,
    pub password: String,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub risk: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RootLoginRequest {
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    #[serde(default)]
    pub current_password: Option<String>,
    pub new_password: String,
}

#[derive(Debug, Deserialize)]
pub struct GithubCallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
}

// ── Helpers ────────────────────────────────────────────────────

fn user_summary(user: &User) -> UserSummary {
    UserSummary {
        id: user.id,
        phira_id: user.phira_id,
        username: user.username_cache.clone(),
        avatar: user.avatar_cache.clone(),
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

fn validate_return_to(allowlist: &[String], return_to: &str) -> Result<(), ApiError> {
    if return_to.is_empty() {
        return Ok(());
    }
    if allowlist.iter().any(|u| u == return_to) {
        Ok(())
    } else {
        Err(ApiError::validation("return_to 不在白名单内"))
    }
}

/// Issue the three auth cookies (access, refresh, csrf).
fn issue_cookies(
    cfg: &crate::config::SessionConfig,
    access_token: &str,
    refresh_token: &str,
) -> (axum::http::HeaderValue, axum::http::HeaderValue, axum::http::HeaderValue) {
    let opts = CookieOpts::new(&cfg.cookie_domain);
    let access = cookies::set_cookie(ACCESS_COOKIE, access_token, &opts, cfg.access_ttl_secs);
    let refresh = cookies::set_cookie(REFRESH_COOKIE, refresh_token, &opts, cfg.refresh_ttl_secs);
    let csrf_opts = CookieOpts::new(&cfg.cookie_domain).http_only(false);
    let csrf_token = new_csrf_token();
    let csrf = cookies::set_cookie(
        &cfg.csrf_cookie_name,
        &csrf_token,
        &csrf_opts,
        cfg.refresh_ttl_secs,
    );
    (access, refresh, csrf)
}

fn new_csrf_token() -> String {
    let mut bytes = [0u8; 24];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build an authenticated response with cookies attached.
fn auth_response(
    body: serde_json::Value,
    cfg: &crate::config::SessionConfig,
    access_token: &str,
    refresh_token: &str,
) -> axum::response::Response {
    let (access_c, refresh_c, csrf_c) = issue_cookies(cfg, access_token, refresh_token);
    let mut resp = (StatusCode::OK, Json(body)).into_response();
    resp.headers_mut().append(header::SET_COOKIE, access_c);
    resp.headers_mut().append(header::SET_COOKIE, refresh_c);
    resp.headers_mut().append(header::SET_COOKIE, csrf_c);
    resp
}

/// Issue an access JWT + session for the given principal.
async fn issue_session_and_jwt(
    state: &Arc<AppState>,
    db: &sqlx::PgPool,
    principal_type: PrincipalType,
    user_id: Option<Uuid>,
    client_type: ClientType,
    device_name: &str,
    ip: &str,
) -> Result<(Session, String, String), ApiError> {
    let refresh_token = session::generate_refresh_token();
    let refresh_hash = session::hash_refresh_token(&refresh_token);
    let sess = session::create_session(
        db,
        principal_type,
        user_id,
        client_type,
        &refresh_hash,
        state.config.session.refresh_ttl_secs,
        device_name,
        ip,
    )
    .await?;
    let sub = user_id.unwrap_or(ROOT_SUB);
    let claims = jwt::AccessClaims::new(
        sub,
        sess.id,
        principal_type,
        client_type,
        state.config.session.access_ttl_secs,
    );
    let access_token = jwt::encode_access(&claims, &state.secrets.jwt_secret)?;
    Ok((sess, access_token, refresh_token))
}

/// Verify a reauth context header for high-risk actions.
pub fn check_reauth_header(
    state: &Arc<AppState>,
    auth: &AuthPrincipal,
    headers: &HeaderMap,
    risk: ReauthRisk,
) -> Result<(), ApiError> {
    let token = headers
        .get("x-reauth-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::new(ErrorCode::Session, "reauth context required"))?;
    let claims = reauth::decode_reauth(token, &state.secrets.jwt_secret, auth.sid)?;
    let adequate = match risk {
        ReauthRisk::Critical => claims.risk == "critical",
        ReauthRisk::High => matches!(claims.risk.as_str(), "high" | "critical"),
    };
    if !adequate {
        return Err(ApiError::new(ErrorCode::Session, "reauth risk insufficient"));
    }
    Ok(())
}

// ── Phira login ────────────────────────────────────────────────

pub async fn phira_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PhiraLoginRequest>,
) -> Result<axum::response::Response, ApiError> {
    let limiter: &RateLimiter = &state.rate_limiter;
    limiter.check(&format!("login:{}", ip_from_headers(&headers)), state.config.rate_limit.login_per_minute)?;

    let client_type = ClientType::parse(body.client_type.as_deref().unwrap_or("ppf"))
        .ok_or_else(|| ApiError::validation("invalid client_type"))?;
    if let Some(rt) = &body.return_to {
        validate_return_to(&state.config.security.return_to_allowlist, rt)?;
    }

    let db = state.require_db()?;
    let (login, me) = phira_service::authenticate_phira(state.phira.as_ref(), &body.email, &body.password)
        .await
        .map_err(phira_service::phira_error_to_api)?;
    let user = phira_service::commit_login(db, &state.credential_cipher, &login, &me).await?;

    let (_sess, access_token, refresh_token) = issue_session_and_jwt(
        &state,
        db,
        PrincipalType::User,
        Some(user.id),
        client_type,
        body.device_name.as_deref().unwrap_or(""),
        &ip_from_headers(&headers),
    )
    .await?;

    let body_json = serde_json::json!({ "user": user_summary(&user) });
    Ok(auth_response(body_json, &state.config.session, &access_token, &refresh_token))
}

// ── Reauth ─────────────────────────────────────────────────────

pub async fn phira_reauth(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
    headers: HeaderMap,
    Json(body): Json<ReauthRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.rate_limiter.check(
        &format!("reauth:{}", ip_from_headers(&headers)),
        state.config.rate_limit.reauth_per_minute,
    )?;

    let db = state.require_db()?;
    let risk = match body.risk.as_deref().unwrap_or("high") {
        "critical" => ReauthRisk::Critical,
        _ => ReauthRisk::High,
    };

    if auth.is_root() {
        RootAuthService::verify(db, &body.password).await?;
    } else {
        let email = body
            .email
            .ok_or_else(|| ApiError::validation("email required for phira reauth"))?;
        phira_service::authenticate_phira(state.phira.as_ref(), &email, &body.password)
            .await
            .map_err(phira_service::phira_error_to_api)?;
    }

    let claims = ReauthClaims::new(
        auth.sub,
        auth.sid,
        auth.principal_type,
        auth.client_type,
        risk,
        state.config.session.reauth_ttl_secs,
    );
    let token = reauth::encode_reauth(&claims, &state.secrets.jwt_secret)?;
    Ok(Json(serde_json::json!({
        "reauth_token": token,
        "expires_in": state.config.session.reauth_ttl_secs,
        "risk": risk.as_str(),
    })))
}

// ── Refresh / logout ───────────────────────────────────────────

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    let refresh_token = cookies::get_cookie(&headers, REFRESH_COOKIE)
        .ok_or_else(|| ApiError::new(ErrorCode::Session, "missing refresh cookie"))?;
    let refresh_hash = session::hash_refresh_token(&refresh_token);
    let sess = session::find_active_by_refresh_hash(db, &refresh_hash)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::Session, "refresh token invalid or expired"))?;

    let principal_type = parse_principal_type(&sess);
    let client_type = ClientType::parse(&sess.client_type).unwrap_or(ClientType::Ppf);

    // Phira credential reauth gate for ordinary users.
    if principal_type == PrincipalType::User {
        if let Some(user_id) = sess.user_id {
            if let Some((_, refresh_expires_at, state_str)) =
                identities_repo::load_phira_credential(db, user_id).await?
            {
                if phira_service::refresh_token_expired(&refresh_expires_at, &chrono::Utc::now())
                    || state_str != "active"
                {
                    identities_repo::mark_reauth_required(db, user_id).await?;
                    return Err(ApiError::new(
                        ErrorCode::PhiraReauthRequired,
                        "需要重新验证 Phira 身份",
                    ));
                }
            }
        }
    }

    // Rotate the refresh token.
    let new_refresh = session::generate_refresh_token();
    let new_hash = session::hash_refresh_token(&new_refresh);
    sqlx::query("UPDATE sessions SET refresh_hash = $1, last_seen_at = now() WHERE id = $2")
        .bind(&new_hash)
        .bind(sess.id)
        .execute(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "refresh rotation failed");
            ApiError::internal()
        })?;

    let sub = sess.user_id.unwrap_or(ROOT_SUB);
    let claims = jwt::AccessClaims::new(
        sub,
        sess.id,
        principal_type,
        client_type,
        state.config.session.access_ttl_secs,
    );
    let access_token = jwt::encode_access(&claims, &state.secrets.jwt_secret)?;

    let body_json = serde_json::json!({ "ok": true });
    Ok(auth_response(body_json, &state.config.session, &access_token, &new_refresh))
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    if let Some(rt) = cookies::get_cookie(&headers, REFRESH_COOKIE) {
        let hash = session::hash_refresh_token(&rt);
        if let Some(sess) = session::find_active_by_refresh_hash(db, &hash).await? {
            session::revoke(db, sess.id).await?;
        }
    } else if let Some(at) = cookies::get_cookie(&headers, ACCESS_COOKIE) {
        if let Ok(claims) = jwt::decode_access(&at, &state.secrets.jwt_secret) {
            session::revoke(db, claims.sid).await?;
        }
    }

    let cfg = &state.config.session;
    let opts = CookieOpts::new(&cfg.cookie_domain);
    let clear_access = cookies::clear_cookie(ACCESS_COOKIE, &opts);
    let clear_refresh = cookies::clear_cookie(REFRESH_COOKIE, &opts);
    let csrf_opts = CookieOpts::new(&cfg.cookie_domain).http_only(false);
    let clear_csrf = cookies::clear_cookie(&cfg.csrf_cookie_name, &csrf_opts);

    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().append(header::SET_COOKIE, clear_access);
    resp.headers_mut().append(header::SET_COOKIE, clear_refresh);
    resp.headers_mut().append(header::SET_COOKIE, clear_csrf);
    Ok(resp)
}

fn parse_principal_type(sess: &Session) -> PrincipalType {
    if sess.principal_type == "root" {
        PrincipalType::Root
    } else {
        PrincipalType::User
    }
}

// ── GitHub bind-only OAuth ─────────────────────────────────────

pub async fn github_start(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    if !state.secrets.github_configured() {
        return Err(ApiError::new(ErrorCode::Auth, "GitHub OAuth not configured"));
    }
    let (url, state_token) = state.github.authorize_url(&state.secrets, &state.config)?;
    state
        .github
        .bind_state_to_user(&state_token, auth.sub, &auth.client_type.to_string());
    Ok(Json(serde_json::json!({ "authorize_url": url, "state": state_token })))
}

pub async fn github_callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<GithubCallbackParams>,
) -> Result<axum::response::Response, ApiError> {
    state.rate_limiter.check(
        &format!("github-callback:{}", params.code.as_deref().unwrap_or("")),
        state.config.rate_limit.github_callback_per_minute,
    )?;

    let code = params
        .code
        .ok_or_else(|| ApiError::new(ErrorCode::Auth, "missing code"))?;
    let state_token = params
        .state
        .ok_or_else(|| ApiError::new(ErrorCode::Auth, "missing state"))?;

    // consume_state enforces the token was bound to an authenticated user.
    let oauth_state = state.github.consume_state(&state_token)?;
    let gh_user = state.github.exchange_code(&state.secrets, &code).await?;

    let db = state.require_db()?;
    let gh_id = gh_user.id.to_string();
    if let Some(existing) = identities_repo::find_by_provider(db, "github", &gh_id).await? {
        if existing.user_id != oauth_state.user_id {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "GitHub 账户已绑定到其他用户",
            ));
        }
    }
    identities_repo::bind_github(db, oauth_state.user_id, &gh_id, &gh_user.login).await?;

    let redirect = format!("{}/auth?github=bound", state.config.site.ppf_url);
    let mut resp = StatusCode::FOUND.into_response();
    resp.headers_mut().insert(
        header::LOCATION,
        header::HeaderValue::from_str(&redirect)
            .map_err(|_| ApiError::validation("invalid redirect"))?,
    );
    Ok(resp)
}

pub async fn github_unbind(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
) -> Result<StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let identities = identities_repo::list_for_user(db, auth.sub).await?;
    for idn in identities {
        if idn.provider == "github" {
            identities_repo::unbind_github(db, auth.sub, &idn.provider_id).await?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

// ── Root ───────────────────────────────────────────────────────

pub async fn root_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RootLoginRequest>,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    let outcome = RootAuthService::verify(db, &body.password).await?;

    let (_sess, access_token, refresh_token) = issue_session_and_jwt(
        &state,
        db,
        PrincipalType::Root,
        None,
        ClientType::Panel,
        "panel-root",
        &ip_from_headers(&headers),
    )
    .await?;

    let body_json = serde_json::json!({
        "must_change_password": outcome.must_change_password,
    });
    Ok(auth_response(body_json, &state.config.session, &access_token, &refresh_token))
}

pub async fn root_change_password(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<StatusCode, ApiError> {
    if !auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let must_change = RootAuthService::must_change_password(db).await?;
    if !must_change {
        let current = body
            .current_password
            .ok_or_else(|| ApiError::validation("current_password required"))?;
        // Re-verify current password unless force-change flow is active.
        RootAuthService::verify(db, &current).await?;
    }
    RootAuthService::change_password(db, &body.new_password).await?;
    Ok(StatusCode::NO_CONTENT)
}
