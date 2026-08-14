//! PPB access JWT.
//!
//! JWT carries only `{sub, sid, principal_type, client_type, iat, exp}`.
//! Permissions are resolved at runtime from the Permission Resolver — never
//! baked into the token (design §6.3).

use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::types::{ClientType, PrincipalType};
use crate::error::{ApiError, ErrorCode};

/// Sentinel `sub` for Root (Root is not in `users`).
pub const ROOT_SUB: Uuid = Uuid::nil();

/// Payload of the PPB access JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: Uuid,
    pub sid: Uuid,
    #[serde(rename = "principal_type")]
    pub principal_type: PrincipalType,
    #[serde(rename = "client_type")]
    pub client_type: ClientType,
    pub iat: i64,
    pub exp: i64,
}

impl AccessClaims {
    pub fn new(
        sub: Uuid,
        sid: Uuid,
        principal_type: PrincipalType,
        client_type: ClientType,
        ttl_secs: i64,
    ) -> Self {
        let now = Utc::now().timestamp();
        Self {
            sub,
            sid,
            principal_type,
            client_type,
            iat: now,
            exp: now + ttl_secs,
        }
    }

    pub fn expired(&self) -> bool {
        Utc::now().timestamp() >= self.exp
    }
}

/// Sign an access token.
pub fn encode_access(claims: &AccessClaims, secret: &str) -> Result<String, ApiError> {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|error| { tracing::error!(%error, "JWT encode failed"); ApiError::internal() })
}

/// Verify and decode an access token.
pub fn decode_access(token: &str, secret: &str) -> Result<AccessClaims, ApiError> {
    // `sub` is a UUID string for users or the nil UUID for root; we validate
    // the token signature/expiry, not the subject value.
    let validation = Validation::new(Algorithm::HS256);
    decode::<AccessClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|_| ApiError::new(ErrorCode::SessionExpired, "invalid or expired access token"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-secret-test-secret-test-secret!!";

    #[test]
    fn round_trip_claims() {
        let claims = AccessClaims::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            PrincipalType::User,
            ClientType::Ppf,
            3600,
        );
        let token = encode_access(&claims, SECRET).unwrap();
        let decoded = decode_access(&token, SECRET).unwrap();
        assert_eq!(decoded.sub, claims.sub);
        assert_eq!(decoded.sid, claims.sid);
        assert_eq!(decoded.principal_type, PrincipalType::User);
        assert_eq!(decoded.client_type, ClientType::Ppf);
        assert!(decoded.exp > decoded.iat);
    }

    #[test]
    fn rejects_tampered_token() {
        let claims = AccessClaims::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            PrincipalType::User,
            ClientType::Ppf,
            3600,
        );
        let token = encode_access(&claims, SECRET).unwrap();
        let mut chars: Vec<char> = token.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == 'a' { 'b' } else { 'a' };
        let tampered: String = chars.into_iter().collect();
        assert!(decode_access(&tampered, SECRET).is_err());
    }

    #[test]
    fn wrong_secret_fails() {
        let claims = AccessClaims::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            PrincipalType::Root,
            ClientType::Panel,
            3600,
        );
        let token = encode_access(&claims, SECRET).unwrap();
        assert!(decode_access(&token, "other-secret").is_err());
    }
}
