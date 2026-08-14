//! GitHub OAuth (bind-only login; design §6.7).
//!
//! Rules:
//! - MUST only bind to an existing Phira-created PPB User (state captures the
//!   authenticated user at `/auth/github/start`).
//! - MUST NOT create bare accounts.
//! - Callback URL comes from validated runtime config and must match the OAuth app.
//! - Default scope: `read:user` (no GitHub API token stored beyond the exchange).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use dashmap::DashMap;
use serde::Deserialize;
use uuid::Uuid;

use crate::config::deployment::Secrets;
use crate::error::{ApiError, ErrorCode};

/// GitHub authorize endpoint.
pub const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
/// GitHub token endpoint.
pub const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
/// GitHub API user endpoint.
pub const GITHUB_API_USER_URL: &str = "https://api.github.com/user";

const STATE_TTL_SECS: u64 = 600;
const MAX_PENDING_STATES: usize = 2048;

/// What `/auth/github/start` captures — binds the callback to a specific user.
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub user_id: Option<Uuid>,
    pub client_type: String,
    pub mode: String,
    pub return_to: String,
    pub accepted_legal: bool,
    pub terms_version: String,
    pub privacy_version: String,
    pub created_at: SystemTime,
}

/// GitHub user info returned from the API.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubUser {
    pub id: i64,
    pub login: String,
}

/// GitHub OAuth service for authenticated bind and already-bound account login.
#[derive(Clone)]
pub struct GithubService {
    http: reqwest::Client,
    states: Arc<DashMap<String, OAuthState>>,
}

#[allow(clippy::new_without_default)]
impl GithubService {
    pub fn new(timeout_ms: u64) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("reqwest client builds");
        Self {
            http,
            states: Arc::new(DashMap::new()),
        }
    }

    /// Build the authorization URL and allocate a short-lived server-owned state record.
    pub fn authorize_url(&self, secrets: &Secrets, cfg: &crate::config::RuntimeConfig) -> Result<(String, String), ApiError> {
        let client_id = secrets
            .github_client_id
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::GithubOauthNotConfigured, "GitHub OAuth not configured"))?;
        self.prune_states();
        if self.states.len() >= MAX_PENDING_STATES {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many pending OAuth states",
            ));
        }
        let state = new_state_token();
        let now = SystemTime::now();
        self.states.insert(
            state.clone(),
            OAuthState {
                user_id: None,
                client_type: String::new(),
                mode: "pending".to_string(),
                return_to: "/".to_string(),
                accepted_legal: false,
                terms_version: String::new(),
                privacy_version: String::new(),
                created_at: now,
            },
        );
        let url = format!(
            "{GITHUB_AUTHORIZE_URL}?client_id={}&redirect_uri={}&scope=read:user&state={}",
            client_id,
            urlencode(&cfg.github.callback_url),
            state
        );
        Ok((url, state))
    }

    /// Attach the current authenticated user to a state token (start handler).
    pub fn bind_state_to_user(&self, state: &str, user_id: Uuid, client_type: &str) {
        if let Some(mut s) = self.states.get_mut(state) {
            s.user_id = Some(user_id);
            s.client_type = client_type.to_string();
            s.mode = "bind".to_string();
            s.return_to = "/profile".to_string();
        }
    }

    /// Configure an unauthenticated GitHub login state. It may only resolve
    /// to an already-bound PPB user at callback time; no bare user is created.
    pub fn mark_login_state(
        &self,
        state: &str,
        return_to: &str,
        client_type: &str,
        accepted_legal: bool,
        terms_version: &str,
        privacy_version: &str,
    ) {
        if let Some(mut s) = self.states.get_mut(state) {
            s.user_id = None;
            s.client_type = client_type.to_string();
            s.mode = "login".to_string();
            s.return_to = return_to.to_string();
            s.accepted_legal = accepted_legal;
            s.terms_version = terms_version.to_string();
            s.privacy_version = privacy_version.to_string();
        }
    }

    /// Verify a state token and return the bound user.
    pub fn consume_state(&self, state: &str) -> Result<OAuthState, ApiError> {
        self.prune_states();
        let entry = self
            .states
            .remove(state)
            .ok_or_else(|| ApiError::new(ErrorCode::GithubOauthStateInvalid, "GitHub OAuth state is invalid or expired"))?;
        Ok(entry.1)
    }

    /// Exchange the callback `code` for a GitHub user (id + login).
    pub async fn exchange_code(
        &self,
        secrets: &Secrets,
        code: &str,
        callback_url: &str,
    ) -> Result<GithubUser, ApiError> {
        let client_id = secrets
            .github_client_id
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::GithubOauthNotConfigured, "GitHub OAuth not configured"))?;
        let client_secret = secrets
            .github_client_secret
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::GithubOauthNotConfigured, "GitHub OAuth not configured"))?;

        // GitHub token endpoint returns form-encoded by default.
        let resp = self
            .http
            .post(GITHUB_TOKEN_URL)
            .form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("code", code),
                ("redirect_uri", callback_url),
            ])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| { tracing::warn!(error=%e, "GitHub OAuth token endpoint unavailable"); ApiError::new(ErrorCode::GithubApiUnavailable, "GitHub is temporarily unavailable") })?;

        let text = resp
            .text()
            .await
            .map_err(|e| { tracing::warn!(error=%e, "GitHub OAuth token response read failed"); ApiError::new(ErrorCode::GithubApiUnavailable, "GitHub is temporarily unavailable") })?;
        let token_body: TokenResponse = serde_json::from_str(&text)
            .map_err(|_| ApiError::new(ErrorCode::GithubOauthFailed, "GitHub OAuth exchange failed"))?;
        if let Some(err) = token_body.error {
            tracing::warn!(provider_error=%err, "GitHub OAuth provider returned an error");
            return Err(ApiError::new(
                ErrorCode::GithubOauthFailed,
                "GitHub OAuth exchange failed",
            ));
        }
        let access_token = token_body
            .access_token
            .ok_or_else(|| ApiError::new(ErrorCode::GithubOauthFailed, "GitHub OAuth exchange failed"))?;

        let user = self
            .http
            .get(GITHUB_API_USER_URL)
            .bearer_auth(&access_token)
            .header(reqwest::header::USER_AGENT, "phira-plus-backend")
            .send()
            .await
            .map_err(|e| { tracing::warn!(error=%e, "GitHub user API unavailable"); ApiError::new(ErrorCode::GithubApiUnavailable, "GitHub is temporarily unavailable") })?
            .json::<GithubUser>()
            .await
            .map_err(|e| { tracing::warn!(error=%e, "GitHub user response invalid"); ApiError::new(ErrorCode::GithubOauthFailed, "GitHub returned an invalid OAuth response") })?;

        Ok(user)
    }

    fn prune_states(&self) {
        let now = SystemTime::now();
        self.states
            .retain(|_, s| now.duration_since(s.created_at).map(|d| d < Duration::from_secs(STATE_TTL_SECS)).unwrap_or(false));
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

fn new_state_token() -> String {
    let mut bytes = [0u8; 24];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}

fn urlencode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}
