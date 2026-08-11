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
    #[serde(rename = "resourceKey")]
    pub resource_key: String,
    #[serde(rename = "argumentsRedacted")]
    pub arguments_redacted: serde_json::Value,
    pub status: String,
    #[serde(rename = "startedAt")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(rename = "finishedAt")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(rename = "resultSummary")]
    pub result_summary: String,
    #[serde(rename = "errorCode")]
    pub error_code: String,
}
