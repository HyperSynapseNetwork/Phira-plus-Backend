//! GitHub OAuth (bind-only login; design §6.7).
//!
//! Rules:
//! - MUST only bind to an existing Phira-created PPB User (state captures the
//!   authenticated user at `/auth/github/start`).
//! - MUST NOT create bare accounts.
//! - Callback URL is fixed: `https://api-phira.htadiy.com/api/v1/auth/github/callback`.
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

/// What `/auth/github/start` captures — binds the callback to a specific user.
#[derive(Debug, Clone)]
pub struct OAuthState {
    pub user_id: Uuid,
    pub client_type: String,
    pub created_at: SystemTime,
}

/// GitHub user info returned from the API.
#[derive(Debug, Clone, Deserialize)]
pub struct GithubUser {
    pub id: i64,
    pub login: String,
}

/// Bind-only GitHub OAuth service.
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

    /// Build the authorization URL. Stores state bound to the current user.
    pub fn authorize_url(&self, secrets: &Secrets, cfg: &crate::config::RuntimeConfig) -> Result<(String, String), ApiError> {
        let client_id = secrets
            .github_client_id
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::Auth, "GitHub OAuth not configured"))?;
        let state = new_state_token();
        let now = SystemTime::now();
        self.states.insert(
            state.clone(),
            OAuthState {
                user_id: Uuid::nil(), // filled in by caller via `bind_state_to_user`
                client_type: "ppf".to_string(),
                created_at: now,
            },
        );
        self.prune_states();
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
            s.user_id = user_id;
            s.client_type = client_type.to_string();
        }
    }

    /// Verify a state token and return the bound user.
    pub fn consume_state(&self, state: &str) -> Result<OAuthState, ApiError> {
        self.prune_states();
        let entry = self
            .states
            .remove(state)
            .ok_or_else(|| ApiError::new(ErrorCode::Session, "invalid or expired OAuth state"))?;
        if entry.1.user_id == Uuid::nil() {
            return Err(ApiError::new(
                ErrorCode::Session,
                "OAuth state not bound to an authenticated user",
            ));
        }
        Ok(entry.1)
    }

    /// Exchange the callback `code` for a GitHub user (id + login).
    pub async fn exchange_code(
        &self,
        secrets: &Secrets,
        code: &str,
    ) -> Result<GithubUser, ApiError> {
        let client_id = secrets
            .github_client_id
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::Auth, "GitHub OAuth not configured"))?;
        let client_secret = secrets
            .github_client_secret
            .clone()
            .ok_or_else(|| ApiError::new(ErrorCode::Auth, "GitHub OAuth not configured"))?;

        // GitHub token endpoint returns form-encoded by default.
        let resp = self
            .http
            .post(GITHUB_TOKEN_URL)
            .form(&[
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("code", code),
                ("redirect_uri", "https://api-phira.htadiy.com/api/v1/auth/github/callback"),
            ])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("github token: {e}")))?;

        let text = resp
            .text()
            .await
            .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("github token body: {e}")))?;
        let token_body: TokenResponse = serde_json::from_str(&text)
            .map_err(|_| ApiError::new(ErrorCode::Auth, "GitHub OAuth exchange failed"))?;
        if let Some(err) = token_body.error {
            return Err(ApiError::new(
                ErrorCode::Auth,
                format!("GitHub OAuth error: {err}"),
            ));
        }
        let access_token = token_body
            .access_token
            .ok_or_else(|| ApiError::new(ErrorCode::Auth, "GitHub OAuth access_token missing"))?;

        let user = self
            .http
            .get(GITHUB_API_USER_URL)
            .bearer_auth(&access_token)
            .header(reqwest::header::USER_AGENT, "phira-plus-backend")
            .send()
            .await
            .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("github user: {e}")))?
            .json::<GithubUser>()
            .await
            .map_err(|e| ApiError::new(ErrorCode::PhiraApiUnavailable, format!("github user parse: {e}")))?;

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
