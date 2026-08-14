//! Phira password login flow (design §6.1/§6.2).
//!
//! `email/password` → Phira `/login` + `/me` → upsert PPB User + Phira identity,
//! encrypted refresh token, and default group membership. The password is used
//! transiently only; it is never persisted or logged.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};
use crate::identities::repo as identities_repo;
use crate::permissions::groups;
use crate::phira::client::{PhiraApi, PhiraError, PhiraLoginResponse, PhiraMe};
use crate::phira::credential::CredentialCipher;
use crate::users::repo as users_repo;
use crate::users::model::User;

/// Success of the Phira password flow.
#[derive(Debug, Clone)]
pub struct LoginSuccess {
    pub user: User,
    pub phira_login: PhiraLoginResponse,
}

/// Pure step: authenticate against Phira. Testable with a mock PhiraApi.
pub async fn authenticate_phira(
    phira: &dyn PhiraApi,
    email: &str,
    password: &str,
) -> Result<(PhiraLoginResponse, PhiraMe), PhiraError> {
    let login = phira.login_email(email, password).await?;
    let me = phira.me(&login.token).await?;
    Ok((login, me))
}

/// Commit the login to PPB state (users/identity/credential/groups). Returns the
/// resulting user. No secrets are logged.
pub async fn commit_login(
    db: &sqlx::PgPool,
    cipher: &CredentialCipher,
    login: &PhiraLoginResponse,
    me: &PhiraMe,
) -> Result<User, ApiError> {
    // Credential state must stay in sync: reauth_required -> active.
    let user = users_repo::upsert_by_phira_id(db, login.id, &me.username, &me.avatar).await?;
    identities_repo::upsert_phira_identity(db, user.id, login.id, &me.username).await?;

    let refresh_expires_at = parse_expire_at(&login.expire_at)
        .unwrap_or_else(|_| Utc::now() + chrono::Duration::days(30));
    let ct = cipher.encrypt(login.refresh_token.as_bytes())?;
    identities_repo::store_phira_credential(db, user.id, &ct, refresh_expires_at).await?;

    groups::ensure_user_in_default_group(db, user.id).await?;
    Ok(user)
}

/// Determine whether a refresh token is still usable.
pub fn refresh_token_expired(refresh_expires_at: &DateTime<Utc>, now: &DateTime<Utc>) -> bool {
    *refresh_expires_at <= *now
}

/// Parse Phira's `expireAt` (RFC3339). Falls back to error.
pub fn parse_expire_at(value: &str) -> Result<DateTime<Utc>, ApiError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| ApiError::new(ErrorCode::ValidationFailed, "invalid expireAt"))
}

/// Map a PhiraError into the correct ApiError (reauth vs unavailable vs auth).
pub fn phira_error_to_api(e: PhiraError) -> ApiError {
    match e {
        PhiraError::InvalidCredentials => ApiError::new(ErrorCode::PhiraAuthFailed, "invalid credentials"),
        PhiraError::ReauthRequired(m) => ApiError::with_details(
            ErrorCode::PhiraReauthRequired,
            "需要重新验证 Phira 身份",
            serde_json::json!({ "reason": m }),
        ),
        PhiraError::Api(m) | PhiraError::Other(m) => {
            ApiError::new(ErrorCode::PhiraApiUnavailable, m)
        }
        PhiraError::Unavailable(m) => ApiError::new(ErrorCode::PhiraApiUnavailable, m),
        PhiraError::RateLimited => ApiError::new(ErrorCode::RateLimited, "phira api rate limited"),
    }
}

/// Extract the plaintext refresh token from a credential row (for refresh flow).
/// The ciphertext is decrypted in memory only.
pub async fn decrypt_refresh_token(
    db: &sqlx::PgPool,
    cipher: &CredentialCipher,
    user_id: Uuid,
) -> Result<(String, DateTime<Utc>, String), ApiError> {
    let row = identities_repo::load_phira_credential(db, user_id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::PhiraReauthRequired, "no phira credential"))?;
    let (ct, refresh_expires_at, state) = row;
    if state != "active" {
        return Err(ApiError::new(ErrorCode::PhiraReauthRequired, "phira credential not active"));
    }
    let plaintext = cipher.decrypt(&ct)?;
    let token = String::from_utf8(plaintext)
        .map_err(|_| ApiError::new(ErrorCode::InternalError, "credential decode failed"))?;
    Ok((token, refresh_expires_at, state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phira::client::MockPhiraApi;

    #[tokio::test]
    async fn authenticate_success() {
        let mock = MockPhiraApi { user_id: 7, ..Default::default() };
        let (login, me) = authenticate_phira(&mock, "a@b.c", "pw").await.unwrap();
        assert_eq!(login.id, 7);
        assert_eq!(me.username, "alice");
    }

    #[tokio::test]
    async fn authenticate_failure_maps_to_auth() {
        let mock = MockPhiraApi { fail_login: true, ..Default::default() };
        let err = authenticate_phira(&mock, "a@b.c", "wrong").await.unwrap_err();
        let api = phira_error_to_api(err);
        assert_eq!(api.code, ErrorCode::AuthRequired);
    }

    #[tokio::test]
    async fn refresh_expired_maps_to_reauth() {
        let mock = MockPhiraApi { refresh_invalid: true, ..Default::default() };
        let err = mock.login_refresh("stale").await.unwrap_err();
        let api = phira_error_to_api(err);
        assert_eq!(api.code, ErrorCode::PhiraReauthRequired);
    }

    #[test]
    fn refresh_expiry_check() {
        let now = Utc::now();
        let expired = now - chrono::Duration::seconds(1);
        let valid = now + chrono::Duration::days(1);
        assert!(refresh_token_expired(&expired, &now));
        assert!(!refresh_token_expired(&valid, &now));
    }

    #[test]
    fn parse_expire_at_ok() {
        let dt = parse_expire_at("2026-09-11T00:00:00Z").unwrap();
        assert_eq!(dt, DateTime::parse_from_rfc3339("2026-09-11T00:00:00Z").unwrap().with_timezone(&Utc));
    }
}
