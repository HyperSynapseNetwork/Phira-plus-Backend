//! Auth + Root HTTP routes.

use std::sync::Arc;

use axum::extract::State;
use axum::http::header;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::consent;
use super::jwt::{self, ROOT_SUB};
use super::reauth::{self, ReauthClaims, ReauthRisk};
use super::root::RootAuthService;
use super::session::{self, Session};
use super::types::{AuthPrincipal, ClientType, PrincipalType};
use super::{phira as phira_service, ACCESS_COOKIE, REFRESH_COOKIE};
use crate::app::AppState;
use crate::error::extractors::{ApiJson, ApiQuery};
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};
use crate::identities::repo as identities_repo;
use crate::middleware::cookies::{self, CookieOpts};
use crate::middleware::rate_limit::RateLimiter;
use crate::users::model::User;
use crate::users::repo as users_repo;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/phira/login", post(phira_login))
        .route("/phira/reauth", post(phira_reauth))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/github/start", get(github_start))
        .route("/github/login/start", get(github_login_start))
        .route("/github/callback", get(github_callback))
        .route("/github/unbind", post(github_unbind))
}

pub fn root_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/root/login", post(root_login))
        .route("/auth/root/session", get(root_session))
        .route("/auth/root/change-password", post(root_change_password))
}

// ── Request / response bodies ─────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PhiraLoginRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub return_to: Option<String>,
    #[serde(default)]
    pub accepted: Option<bool>,
    #[serde(default)]
    pub terms_version: Option<String>,
    #[serde(default)]
    pub privacy_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: Uuid,
    pub phira_id: i64,
    pub username: String,
    pub avatar: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ReauthRequest {
    #[serde(default)]
    pub email: Option<String>,
    pub password: String,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub risk: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RootLoginRequest {
    pub password: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ChangePasswordRequest {
    #[serde(default)]
    pub current_password: Option<String>,
    pub new_password: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct GithubLoginStartParams {
    #[serde(default)]
    pub return_to: Option<String>,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub accepted: Option<bool>,
    #[serde(default)]
    pub terms_version: Option<String>,
    #[serde(default)]
    pub privacy_version: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GithubCallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
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
    // Accept an exact whitelisted origin, or a safe relative path (no scheme,
    // no protocol-relative `//`, no backslash, no userinfo `@`).
    if allowlist.iter().any(|u| u == return_to) {
        return Ok(());
    }
    let is_relative = return_to.starts_with('/')
        && !return_to.starts_with("//")
        && !return_to.contains('\\')
        && !return_to.contains('@')
        && !return_to.contains("://");
    if is_relative {
        return Ok(());
    }
    Err(ApiError::validation("return_to 不在白名单内"))
}

/// Issue the two auth cookies (access, refresh). CSRF token is issued via
/// `GET /api/v1/me` (contract §20 S-1) — not a readable cookie.
fn issue_cookies(
    cfg: &crate::config::SessionConfig,
    access_token: &str,
    refresh_token: &str,
) -> (axum::http::HeaderValue, axum::http::HeaderValue) {
    let opts = CookieOpts::from_session(cfg);
    let access = cookies::set_cookie(ACCESS_COOKIE, access_token, &opts, cfg.access_ttl_secs);
    let refresh = cookies::set_cookie(REFRESH_COOKIE, refresh_token, &opts, cfg.refresh_ttl_secs);
    (access, refresh)
}

/// Build an authenticated response with cookies attached.
fn auth_response(
    body: serde_json::Value,
    cfg: &crate::config::SessionConfig,
    access_token: &str,
    refresh_token: &str,
) -> axum::response::Response {
    let (access_c, refresh_c) = issue_cookies(cfg, access_token, refresh_token);
    let mut resp = (StatusCode::OK, Json(body)).into_response();
    resp.headers_mut().append(header::SET_COOKIE, access_c);
    resp.headers_mut().append(header::SET_COOKIE, refresh_c);
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
        .ok_or_else(|| ApiError::new(ErrorCode::SessionExpired, "reauth context required"))?;
    let claims = reauth::decode_reauth(token, &state.secrets.jwt_secret, auth.sid)?;
    let adequate = match risk {
        ReauthRisk::Critical => claims.risk == "critical",
        ReauthRisk::High => matches!(claims.risk.as_str(), "high" | "critical"),
    };
    if !adequate {
        return Err(ApiError::new(ErrorCode::SessionExpired, "reauth risk insufficient"));
    }
    Ok(())
}

// ── Phira login ────────────────────────────────────────────────

/// Phira email/password login (sets access + refresh cookies).
#[utoipa::path(
    post,
    path = "/api/v1/auth/phira/login",
    operation_id = "auth_phira_login_post",
    request_body = PhiraLoginRequest,
    responses(
        (status = 200, description = "logged in; cookies set", body = serde_json::Value),
        (status = 401, description = "invalid credentials", body = ErrorEnvelope),
        (status = 422, description = "current Terms/Privacy versions were not explicitly accepted", body = ErrorEnvelope),
        (status = 503, description = "approved legal documents are not configured", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
pub async fn phira_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<PhiraLoginRequest>,
) -> Result<axum::response::Response, ApiError> {
    let limiter: &RateLimiter = &state.rate_limiter;
    limiter.check(&format!("login:{}", ip_from_headers(&headers)), state.config.rate_limit.login_per_minute)?;

    let client_type = ClientType::parse(body.client_type.as_deref().unwrap_or("ppf"))
        .ok_or_else(|| ApiError::validation("invalid client_type"))?;
    if let Some(rt) = &body.return_to {
        validate_return_to(&state.config.security.return_to_allowlist, rt)?;
    }
    // Public account auth is fail-closed when approved legal documents are not configured.
    consent::current_versions(&state.config.legal)?;

    let db = state.require_db()?;
    let (login, me) = phira_service::authenticate_phira(state.phira.as_ref(), &body.email, &body.password)
        .await
        .map_err(phira_service::phira_error_to_api)?;
    let existing_user = users_repo::find_by_phira_id(db, login.id).await?;
    let legal_acceptance = consent::acceptance_for_login(
        db,
        existing_user.as_ref().map(|user| user.id),
        &state.config.legal,
        body.accepted == Some(true),
        body.terms_version.as_deref(),
        body.privacy_version.as_deref(),
    )
    .await?;
    let user = phira_service::commit_login(db, &state.credential_cipher, &login, &me).await?;
    if let Some(acceptance) = legal_acceptance.as_ref() {
        consent::record_acceptance(db, user.id, client_type, acceptance, "phira_login").await?;
    }

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

/// Issue a short-lived reauth context (`X-Reauth-Token`) for elevated actions.
#[utoipa::path(
    post,
    path = "/api/v1/auth/phira/reauth",
    operation_id = "auth_phira_reauth_post",
    request_body = ReauthRequest,
    responses(
        (status = 200, description = "reauth context issued", body = serde_json::Value),
        (status = 401, description = "invalid credentials / phira_id mismatch", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
pub async fn phira_reauth(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
    headers: HeaderMap,
    ApiJson(body): ApiJson<ReauthRequest>,
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
        // Root uses independent local reauth (S-2).
        RootAuthService::verify(db, &body.password).await?;
    } else {
        let email = body
            .email
            .ok_or_else(|| ApiError::validation("email required for phira reauth"))?;
        let (login, _me) = phira_service::authenticate_phira(state.phira.as_ref(), &email, &body.password)
            .await
            .map_err(phira_service::phira_error_to_api)?;
        // S-2: the reauthenticated Phira identity MUST equal the current user's
        // phira_id; otherwise refuse to issue the elevated context.
        let user = crate::users::repo::find_by_id(db, auth.sub)
            .await?
            .ok_or_else(|| ApiError::not_found("user"))?;
        if login.id != user.phira_id {
            return Err(ApiError::new(
                ErrorCode::AuthRequired,
                "reauth phira_id does not match the current user",
            ));
        }
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

/// Rotate the refresh token and issue a new access token.
#[utoipa::path(
    post,
    path = "/api/v1/auth/refresh",
    operation_id = "auth_refresh_post",
    responses(
        (status = 200, description = "refreshed; cookies rotated", body = serde_json::Value),
        (status = 401, description = "invalid/expired refresh token", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
pub async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    let refresh_token = cookies::get_cookie(&headers, REFRESH_COOKIE)
        .ok_or_else(|| ApiError::new(ErrorCode::SessionExpired, "missing refresh cookie"))?;
    let refresh_hash = session::hash_refresh_token(&refresh_token);
    let sess = session::find_active_by_refresh_hash(db, &refresh_hash)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::SessionExpired, "refresh token invalid or expired"))?;

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

/// Revoke the session and clear cookies.
#[utoipa::path(
    post,
    path = "/api/v1/auth/logout",
    operation_id = "auth_logout_post",
    responses(
        (status = 204, description = "logged out"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
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
    let opts = CookieOpts::from_session(cfg);
    let clear_access = cookies::clear_cookie(ACCESS_COOKIE, &opts);
    let clear_refresh = cookies::clear_cookie(REFRESH_COOKIE, &opts);

    let mut resp = StatusCode::NO_CONTENT.into_response();
    resp.headers_mut().append(header::SET_COOKIE, clear_access);
    resp.headers_mut().append(header::SET_COOKIE, clear_refresh);
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

#[utoipa::path(
    get,
    path = "/api/v1/auth/github/start",
    operation_id = "auth_github_start_get",
    responses((status = 200, description = "GitHub bind authorization URL", body = serde_json::Value), (status = 503, description = "GitHub OAuth not configured", body = ErrorEnvelope)),
    tag = "auth"
)]
pub async fn github_start(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    if !state.secrets.github_configured() {
        return Err(ApiError::new(ErrorCode::GithubOauthNotConfigured, "GitHub OAuth not configured"));
    }
    let network_key = ip_from_headers(&headers);
    state.rate_limiter.check(
        &format!("github-bind-start:{}:{}", auth.sub, network_key),
        state.config.rate_limit.github_start_per_minute,
    )?;
    state.rate_limiter.check(
        "github-provider:start",
        state.config.rate_limit.github_provider_per_minute,
    )?;
    let (url, state_token) = state.github.authorize_url(&state.secrets, &state.config)?;
    state
        .github
        .bind_state_to_user(&state_token, auth.sub, &auth.client_type.to_string());
    Ok(Json(serde_json::json!({ "authorize_url": url, "state": state_token })))
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/github/login/start",
    operation_id = "auth_github_login_start_get",
    params(GithubLoginStartParams),
    responses((status = 302, description = "redirect to GitHub OAuth"), (status = 422, description = "current Terms/Privacy versions were not explicitly accepted", body = ErrorEnvelope), (status = 503, description = "GitHub OAuth or approved legal documents not configured", body = ErrorEnvelope)),
    tag = "auth"
)]
pub async fn github_login_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiQuery(params): ApiQuery<GithubLoginStartParams>,
) -> Result<axum::response::Response, ApiError> {
    if !state.secrets.github_configured() {
        return Err(ApiError::new(ErrorCode::GithubOauthNotConfigured, "GitHub OAuth not configured"));
    }
    let client_type = ClientType::parse(params.client_type.as_deref().unwrap_or("ppf"))
        .ok_or_else(|| ApiError::validation("invalid client_type"))?;
    if !matches!(client_type, ClientType::Ppf | ClientType::Panel) {
        return Err(ApiError::validation("GitHub web login supports ppf/panel clients only"));
    }
    let network_key = ip_from_headers(&headers);
    state.rate_limiter.check(
        &format!("github-login-start:{}:{}", client_type, network_key),
        state.config.rate_limit.github_start_per_minute,
    )?;
    state.rate_limiter.check(
        "github-provider:start",
        state.config.rate_limit.github_provider_per_minute,
    )?;
    let return_to = params.return_to.unwrap_or_else(|| match client_type {
        ClientType::Panel => "/".to_string(),
        _ => "/profile".to_string(),
    });
    validate_return_to(&state.config.security.return_to_allowlist, &return_to)?;
    let current_legal = consent::current_versions(&state.config.legal)?;
    let accepted_legal = params.accepted == Some(true);
    let legal_versions = if accepted_legal {
        consent::validate_acceptance(
            &state.config.legal,
            true,
            params.terms_version.as_deref(),
            params.privacy_version.as_deref(),
        )?
    } else {
        current_legal
    };
    let (url, state_token) = state.github.authorize_url(&state.secrets, &state.config)?;
    state.github.mark_login_state(
        &state_token,
        &return_to,
        &client_type.to_string(),
        accepted_legal,
        &legal_versions.terms_version,
        &legal_versions.privacy_version,
    );
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(header::LOCATION, header::HeaderValue::from_str(&url).map_err(|_| ApiError::validation("invalid GitHub authorize URL"))?);
    Ok(response)
}

fn github_gateway_error_redirect(
    client_type: &str,
    return_to: &str,
    code: ErrorCode,
    request_id: &str,
) -> Result<axum::response::Response, ApiError> {
    let client_type = if client_type == "panel" { "panel" } else { "ppf" };
    let return_to = percent_encoding::utf8_percent_encode(return_to, percent_encoding::NON_ALPHANUMERIC);
    let request_id = percent_encoding::utf8_percent_encode(request_id, percent_encoding::NON_ALPHANUMERIC);
    let location = format!(
        "/auth/phira/login?client_type={client_type}&return_to={return_to}&intent=github&error={}&request_id={request_id}",
        code.as_str()
    );
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(
        header::LOCATION,
        header::HeaderValue::from_str(&location)
            .map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

pub async fn github_callback(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiQuery(params): ApiQuery<GithubCallbackParams>,
) -> Result<axum::response::Response, ApiError> {
    let request_id = crate::middleware::request_id::current_request_id();
    let network_key = ip_from_headers(&headers);
    if let Err(error) = state.rate_limiter.check(
        &format!("github-callback:{}", network_key),
        state.config.rate_limit.github_callback_per_minute,
    ) {
        return github_gateway_error_redirect("ppf", "/profile", error.code, &request_id);
    }
    if let Err(error) = state.rate_limiter.check(
        "github-provider:callback",
        state.config.rate_limit.github_provider_per_minute,
    ) {
        return github_gateway_error_redirect("ppf", "/profile", error.code, &request_id);
    }

    let state_token = match params.state {
        Some(value) if !value.trim().is_empty() => value,
        _ => return github_gateway_error_redirect("ppf", "/profile", ErrorCode::GithubOauthStateInvalid, &request_id),
    };
    let oauth_state = match state.github.consume_state(&state_token) {
        Ok(value) => value,
        Err(error) => return github_gateway_error_redirect("ppf", "/profile", error.code, &request_id),
    };
    if params.error.is_some() {
        return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            ErrorCode::GithubOauthFailed,
            &request_id,
        );
    }
    let code = match params.code {
        Some(value) if !value.trim().is_empty() => value,
        _ => return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            ErrorCode::GithubOauthFailed,
            &request_id,
        ),
    };

    let gh_user = match state
        .github
        .exchange_code(&state.secrets, &code, &state.config.github.callback_url)
        .await
    {
        Ok(user) => user,
        Err(error) => return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            error.code,
            &request_id,
        ),
    };

    let db = match state.require_db() {
        Ok(db) => db,
        Err(error) => return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            error.code,
            &request_id,
        ),
    };
    let gh_id = gh_user.id.to_string();
    let existing = match identities_repo::find_by_provider(db, "github", &gh_id).await {
        Ok(value) => value,
        Err(error) => return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            error.code,
            &request_id,
        ),
    };

    if oauth_state.mode == "login" {
        let identity = match existing {
            Some(identity) => identity,
            None => return github_gateway_error_redirect(
                &oauth_state.client_type,
                &oauth_state.return_to,
                ErrorCode::GithubIdentityNotBound,
                &request_id,
            ),
        };
        let client_type = match ClientType::parse(&oauth_state.client_type) {
            Some(value @ (ClientType::Ppf | ClientType::Panel)) => value,
            _ => return github_gateway_error_redirect(
                "ppf",
                "/profile",
                ErrorCode::GithubOauthStateInvalid,
                &request_id,
            ),
        };
        let acceptance = match consent::acceptance_for_login(
            db,
            Some(identity.user_id),
            &state.config.legal,
            oauth_state.accepted_legal,
            Some(&oauth_state.terms_version),
            Some(&oauth_state.privacy_version),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return github_gateway_error_redirect(
                &oauth_state.client_type,
                &oauth_state.return_to,
                error.code,
                &request_id,
            ),
        };
        if let Some(acceptance) = acceptance.as_ref() {
            if let Err(error) = consent::record_acceptance(db, identity.user_id, client_type, acceptance, "github_login").await {
                return github_gateway_error_redirect(
                    &oauth_state.client_type,
                    &oauth_state.return_to,
                    error.code,
                    &request_id,
                );
            }
        }
        let (_sess, access_token, refresh_token) = match issue_session_and_jwt(
            &state, db, PrincipalType::User, Some(identity.user_id), client_type, "github-web", "",
        ).await {
            Ok(value) => value,
            Err(error) => return github_gateway_error_redirect(
                &oauth_state.client_type,
                &oauth_state.return_to,
                error.code,
                &request_id,
            ),
        };
        let redirect = crate::auth::gateway::resolve_redirect_target(
            &state.config.site,
            &state.config.security,
            &oauth_state.client_type,
            Some(&oauth_state.return_to),
        );
        let (access_c, refresh_c) = issue_cookies(&state.config.session, &access_token, &refresh_token);
        let mut response = StatusCode::FOUND.into_response();
        response.headers_mut().append(header::SET_COOKIE, access_c);
        response.headers_mut().append(header::SET_COOKIE, refresh_c);
        match header::HeaderValue::from_str(&redirect) {
            Ok(location) => response.headers_mut().insert(header::LOCATION, location),
            Err(_) => return github_gateway_error_redirect(
                &oauth_state.client_type,
                &oauth_state.return_to,
                ErrorCode::InternalError,
                &request_id,
            ),
        };
        return Ok(response);
    }

    let user_id = match oauth_state.user_id {
        Some(value) => value,
        None => return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            ErrorCode::GithubOauthStateInvalid,
            &request_id,
        ),
    };
    if let Some(existing) = existing {
        if existing.user_id != user_id {
            return github_gateway_error_redirect(
                &oauth_state.client_type,
                &oauth_state.return_to,
                ErrorCode::ResourceConflict,
                &request_id,
            );
        }
    }
    if let Err(error) = identities_repo::bind_github(db, user_id, &gh_id, &gh_user.login).await {
        return github_gateway_error_redirect(
            &oauth_state.client_type,
            &oauth_state.return_to,
            error.code,
            &request_id,
        );
    }
    let redirect = format!("{}/profile?github=bound", state.config.site.ppf_url.trim_end_matches('/'));
    let mut response = StatusCode::FOUND.into_response();
    match header::HeaderValue::from_str(&redirect) {
        Ok(location) => response.headers_mut().insert(header::LOCATION, location),
        Err(_) => return github_gateway_error_redirect("ppf", "/profile", ErrorCode::InternalError, &request_id),
    };
    Ok(response)
}

#[utoipa::path(
    post,
    path = "/api/v1/auth/github/unbind",
    operation_id = "auth_github_unbind_post",
    responses((status = 204, description = "GitHub identity unbound"), (status = 401, description = "unauthenticated", body = ErrorEnvelope)),
    tag = "auth"
)]
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

/// POST /api/v1/admin/auth/root/login — Root local principal login.
#[utoipa::path(
    post,
    path = "/api/v1/admin/auth/root/login",
    operation_id = "admin_auth_root_login_post",
    request_body = RootLoginRequest,
    responses(
        (status = 200, description = "root logged in; cookies set", body = serde_json::Value),
        (status = 401, description = "invalid password", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
pub async fn root_login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<RootLoginRequest>,
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
        "principal_type": "root",
        "must_change_password": outcome.must_change_password,
    });
    Ok(auth_response(body_json, &state.config.session, &access_token, &refresh_token))
}

/// GET /api/v1/admin/auth/root/session — root session probe (P1).
#[utoipa::path(
    get,
    path = "/api/v1/admin/auth/root/session",
    operation_id = "admin_auth_root_session_get",
    responses(
        (status = 200, description = "root session probe", body = serde_json::Value),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
pub async fn root_session(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let must_change = RootAuthService::must_change_password(db).await?;
    Ok(Json(serde_json::json!({
        "principal_type": "root",
        "user_id": null,
        "session_id": auth.sid,
        "must_change_password": must_change,
    })))
}

/// POST /api/v1/admin/auth/root/change-password — change Root password.
#[utoipa::path(
    post,
    path = "/api/v1/admin/auth/root/change-password",
    operation_id = "admin_auth_root_change_password_post",
    request_body = ChangePasswordRequest,
    responses(
        (status = 204, description = "password changed"),
        (status = 401, description = "invalid current password", body = ErrorEnvelope),
    ),
    tag = "auth"
)]
pub async fn root_change_password(
    State(state): State<Arc<AppState>>,
    auth: AuthPrincipal,
    ApiJson(body): ApiJson<ChangePasswordRequest>,
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
