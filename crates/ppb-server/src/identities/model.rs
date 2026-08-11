//! Identity bindings and Phira credential state.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UserIdentity {
    pub id: Uuid,
    #[serde(rename = "userId")]
    pub user_id: Uuid,
    pub provider: String, // phira | github
    #[serde(rename = "providerId")]
    pub provider_id: String,
    #[serde(rename = "providerName")]
    pub provider_name: String,
    #[serde(rename = "linkedAt")]
    pub linked_at: DateTime<Utc>,
}

/// Phira credential state (encrypted refresh token; state drives reauth).
#[derive(Debug, Clone, Serialize)]
pub struct PhiraCredentialState {
    #[serde(rename = "userId")]
    pub user_id: Uuid,
    /// Never expose the ciphertext. `false` when a credential row exists.
    #[serde(rename = "hasCredential")]
    pub has_credential: bool,
    pub state: String,
    #[serde(rename = "refreshExpiresAt")]
    pub refresh_expires_at: Option<DateTime<Utc>>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<DateTime<Utc>>,
}
