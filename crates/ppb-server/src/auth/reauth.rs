//! Short-lived reauth context (design §6.9).
//!
//! Re-verifying the Phira password (or Root password) issues a 5-minute
//! `reauth_context` JWT bound to {session, principal, client, risk}. High-risk
//! Actions require `X-Reauth-Token` matching the current session.

use chrono::Utc;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::types::{ClientType, PrincipalType};
use crate::error::{ApiError, ErrorCode};

/// Risk levels an action may require for reauth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReauthRisk {
    High,
    Critical,
}

impl ReauthRisk {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Critical => "critical",
        }
    }
}

/// Reauth context claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReauthClaims {
    pub purpose: String, // "reauth"
    pub sub: Uuid,
    pub sid: Uuid,
    #[serde(rename = "principal_type")]
    pub principal_type: PrincipalType,
    #[serde(rename = "client_type")]
    pub client_type: ClientType,
    pub risk: String,
    pub iat: i64,
    pub exp: i64,
}

impl ReauthClaims {
    pub fn new(
        sub: Uuid,
        sid: Uuid,
        principal_type: PrincipalType,
        client_type: ClientType,
        risk: ReauthRisk,
        ttl_secs: i64,
    ) -> Self {
        let now = Utc::now().timestamp();
        Self {
            purpose: "reauth".to_string(),
            sub,
            sid,
            principal_type,
            client_type,
            risk: risk.as_str().to_string(),
            iat: now,
            exp: now + ttl_secs,
        }
    }
}

pub fn encode_reauth(claims: &ReauthClaims, secret: &str) -> Result<String, ApiError> {
    encode(
        &Header::new(Algorithm::HS256),
        claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ApiError::new(ErrorCode::Internal, format!("reauth encode: {e}")))
}

/// Decode and validate a reauth context for the given session.
pub fn decode_reauth(token: &str, secret: &str, expected_sid: Uuid) -> Result<ReauthClaims, ApiError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_sub = false;
    let claims = decode::<ReauthClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|_| ApiError::new(ErrorCode::Session, "invalid reauth context"))?
    .claims;

    if claims.purpose != "reauth" {
        return Err(ApiError::new(ErrorCode::Session, "not a reauth context"));
    }
    if claims.sid != expected_sid {
        return Err(ApiError::new(ErrorCode::Session, "reauth context bound to another session"));
    }
    if Utc::now().timestamp() >= claims.exp {
        return Err(ApiError::new(ErrorCode::Session, "reauth context expired"));
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "reauth-test-secret-reauth-test-secret!";

    #[test]
    fn round_trip_and_binding() {
        let sid = Uuid::new_v4();
        let claims = ReauthClaims::new(
            Uuid::new_v4(),
            sid,
            PrincipalType::User,
            ClientType::Panel,
            ReauthRisk::Critical,
            300,
        );
        let token = encode_reauth(&claims, SECRET).unwrap();
        let decoded = decode_reauth(&token, SECRET, sid).unwrap();
        assert_eq!(decoded.risk, "critical");
        assert_eq!(decoded.sid, sid);
    }

    #[test]
    fn rejects_other_session() {
        let claims = ReauthClaims::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            PrincipalType::User,
            ClientType::Panel,
            ReauthRisk::High,
            300,
        );
        let token = encode_reauth(&claims, SECRET).unwrap();
        assert!(decode_reauth(&token, SECRET, Uuid::new_v4()).is_err());
    }
}
