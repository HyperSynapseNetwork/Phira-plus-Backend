//! Command run model.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct CommandRun {
    pub id: Uuid,
    pub action: String,
    pub actor: String,
    pub resource_key: String,
    pub arguments_redacted: serde_json::Value,
    pub status: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result_summary: String,
    pub error_code: String,
}
