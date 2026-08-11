//! Auth shared types.

use serde::{Deserialize, Serialize};

/// Principal type. Root is a local emergency principal, never in `users`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrincipalType {
    User,
    Root,
}

impl std::fmt::Display for PrincipalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "user"),
            Self::Root => write!(f, "root"),
        }
    }
}

/// Client type carried in sessions and JWTs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientType {
    Ppf,
    Panel,
    Windows,
    Android,
}

impl std::fmt::Display for ClientType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ppf => write!(f, "ppf"),
            Self::Panel => write!(f, "panel"),
            Self::Windows => write!(f, "windows"),
            Self::Android => write!(f, "android"),
        }
    }
}

impl ClientType {
    /// Parse from a wire string (lowercase). Used by login requests.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ppf" => Some(Self::Ppf),
            "panel" => Some(Self::Panel),
            "windows" => Some(Self::Windows),
            "android" => Some(Self::Android),
            _ => None,
        }
    }
}

/// A decoded, authenticated principal (see middleware::auth).
#[derive(Debug, Clone)]
pub struct AuthPrincipal {
    /// PPB user UUID. Meaningful only when `principal_type == User`.
    pub sub: uuid::Uuid,
    /// Session UUID.
    pub sid: uuid::Uuid,
    pub principal_type: PrincipalType,
    pub client_type: ClientType,
    pub request_id: String,
}

impl AuthPrincipal {
    pub fn is_root(&self) -> bool {
        self.principal_type == PrincipalType::Root
    }
}
