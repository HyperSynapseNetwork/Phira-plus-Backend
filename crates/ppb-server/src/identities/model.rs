//! Identity bindings and Phira credential state.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct UserIdentity {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String, // phira | github
    pub provider_id: String,
    pub provider_name: String,
    pub linked_at: DateTime<Utc>,
}

/// Phira credential state (encrypted refresh token; state drives reauth).
#[derive(Debug, Clone, Serialize)]
pub struct PhiraCredentialState {
    pub user_id: Uuid,
    /// Never expose the ciphertext. `false` when a credential row exists.
    pub has_credential: bool,
    pub state: String,
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}
