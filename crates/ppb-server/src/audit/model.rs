//! Audit event model (90-day retention).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct AuditEvent {
    pub id: Uuid,
    pub occurred_at: DateTime<Utc>,
    pub principal_type: String,
    pub actor_user_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub parameters_redacted: serde_json::Value,
    pub result: String,
    pub error_code: String,
    pub request_id: String,
    pub command_id: String,
    pub ip: String,
    pub user_agent: String,
}

/// New audit event (before DB id/occurred_at assigned).
#[derive(Debug, Clone)]
pub struct NewAuditEvent {
    pub principal_type: String,
    pub actor_user_id: Option<Uuid>,
    pub actor_session_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: String,
    pub parameters_redacted: serde_json::Value,
    pub result: String,
    pub error_code: String,
    pub request_id: String,
    pub command_id: String,
    pub ip: String,
    pub user_agent: String,
}
