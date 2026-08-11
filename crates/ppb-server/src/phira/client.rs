//! Phira API client + trait (testable).
//!
//! Facts (audit §2.9): base `https://phira.5wyxi.com`, auth `Authorization:
//! Bearer <access>`. `POST /login {email,password}` or `{refreshToken}` →
//! bare `{id, token, refreshToken, expireAt}`; failures are bare `{"error":"..."}`.
//! `GET /me` → bare user object.

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::error::{ApiError, ErrorCode};

/// Successful Phira login response.
#[derive(Debug, Clone, Deserialize)]
pub struct PhiraLoginResponse {
    pub id: i64,
    pub token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expireAt")]
    pub expire_at: String,
}

/// Phira `/me` fields we cache (never treated as source of truth).
#[derive(Debug, Clone, Deserialize)]
pub struct PhiraMe {
    pub id: i64,
    pub username: String,
    pub avatar: String,
}

/// Domain error from Phira interactions.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PhiraError {
    #[error("phira api error: {0}")]
    Api(String),
    #[error("phira api unavailable: {0}")]
    Unavailable(String),
    #[error("invalid phira credentials")]
    InvalidCredentials,
    #[error("phira reauth required: {0}")]
    ReauthRequired(String),
    #[error("phira error: {0}")]
    Other(String),
}

impl From<PhiraError> for ApiError {
    fn from(e: PhiraError) -> Self {
        match e {
            PhiraError::InvalidCredentials => ApiError::new(ErrorCode::Auth, "invalid credentials"),
            PhiraError::ReauthRequired(m) => ApiError::with_details(
                ErrorCode::PhiraReauthRequired,
                "需要重新验证 Phira 身份",
                serde_json::json!({ "reason": m }),
            ),
            PhiraError::Api(m) | PhiraError::Other(m) => {
                ApiError::new(ErrorCode::PhiraApiUnavailable, m)
            }
            PhiraError::Unavailable(m) => ApiError::new(ErrorCode::PhiraApiUnavailable, m),
        }
    }
}

/// Abstraction over the Phira API so auth flows can be unit-tested.
#[async_trait]
pub trait PhiraApi: Send + Sync {
    async fn login_email(
        &self,
        email: &str,
        password: &str,
    ) -> Result<PhiraLoginResponse, PhiraError>;
    async fn login_refresh(&self, refresh_token: &str)
        -> Result<PhiraLoginResponse, PhiraError>;
    async fn me(&self, access_token: &str) -> Result<PhiraMe, PhiraError>;
}

/// Real HTTP client.
#[derive(Clone)]
pub struct PhiraClient {
    http: reqwest::Client,
    base_url: String,
}

impl PhiraClient {
    pub fn new(base_url: &str, timeout_ms: u64) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("reqwest client: {e}")))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    async fn login_post(&self, body: serde_json::Value) -> Result<PhiraLoginResponse, PhiraError> {
        let url = format!("{}/login", self.base_url);
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| PhiraError::Unavailable(e.to_string()))?;
        parse_login_response(resp).await
    }
}

/// Parse a Phira `/login` response (bare JSON or `{"error":"..."}`).
async fn parse_login_response(
    resp: reqwest::Response,
) -> Result<PhiraLoginResponse, PhiraError> {
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| PhiraError::Unavailable(e.to_string()))?;

    let error = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|parsed| {
            parsed
                .get("error")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    if let Some(err) = error {
        return match status.as_u16() {
            401 => Err(PhiraError::InvalidCredentials),
            _ => Err(PhiraError::ReauthRequired(err)),
        };
    }
    serde_json::from_str::<PhiraLoginResponse>(&text)
        .map_err(|_| PhiraError::Api(format!("unexpected login payload (status {status})")))
}

#[async_trait]
impl PhiraApi for PhiraClient {
    async fn login_email(
        &self,
        email: &str,
        password: &str,
    ) -> Result<PhiraLoginResponse, PhiraError> {
        self.login_post(json!({"email": email, "password": password}))
            .await
    }

    async fn login_refresh(
        &self,
        refresh_token: &str,
    ) -> Result<PhiraLoginResponse, PhiraError> {
        self.login_post(json!({"refreshToken": refresh_token})).await
    }

    async fn me(&self, access_token: &str) -> Result<PhiraMe, PhiraError> {
        let url = format!("{}/me", self.base_url);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| PhiraError::Unavailable(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| PhiraError::Unavailable(e.to_string()))?;
        if !status.is_success() {
            return Err(PhiraError::Api(format!("/me failed: status {status}")));
        }
        serde_json::from_str::<PhiraMe>(&text)
            .map_err(|_| PhiraError::Api("unexpected /me payload".to_string()))
    }
}

/// Mock Phira API for auth-flow tests (crate-visible under `cfg(test)`).
#[cfg(test)]
#[derive(Clone, Default)]
pub struct MockPhiraApi {
    pub fail_login: bool,
    pub fail_me: bool,
    pub refresh_invalid: bool,
    pub user_id: i64,
}

#[cfg(test)]
#[async_trait]
impl PhiraApi for MockPhiraApi {
    async fn login_email(
        &self,
        _email: &str,
        _password: &str,
    ) -> Result<PhiraLoginResponse, PhiraError> {
        if self.fail_login {
            return Err(PhiraError::InvalidCredentials);
        }
        Ok(PhiraLoginResponse {
            id: self.user_id,
            token: "access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            expire_at: "2026-09-11T00:00:00Z".to_string(),
        })
    }

    async fn login_refresh(
        &self,
        _refresh_token: &str,
    ) -> Result<PhiraLoginResponse, PhiraError> {
        if self.refresh_invalid {
            return Err(PhiraError::ReauthRequired("refresh expired".to_string()));
        }
        Ok(PhiraLoginResponse {
            id: self.user_id,
            token: "new-access-token".to_string(),
            refresh_token: "new-refresh-token".to_string(),
            expire_at: "2026-09-11T00:00:00Z".to_string(),
        })
    }

    async fn me(&self, _access_token: &str) -> Result<PhiraMe, PhiraError> {
        if self.fail_me {
            return Err(PhiraError::Api("me failed".to_string()));
        }
        Ok(PhiraMe {
            id: self.user_id,
            username: "alice".to_string(),
            avatar: "".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_login_success_shape() {
        let mock = MockPhiraApi { user_id: 42, ..Default::default() };
        // Trait is async; use tokio runtime in test.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp = rt.block_on(mock.login_email("a@b.c", "pw")).unwrap();
        assert_eq!(resp.id, 42);
        assert_eq!(resp.refresh_token, "refresh-token");
    }

    #[test]
    fn mock_login_failure_maps() {
        let mock = MockPhiraApi { fail_login: true, ..Default::default() };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt.block_on(mock.login_email("a@b.c", "wrong")).unwrap_err();
        assert!(matches!(err, PhiraError::InvalidCredentials));
        let api: ApiError = err.into();
        assert_eq!(api.code, ErrorCode::Auth);
    }
}
