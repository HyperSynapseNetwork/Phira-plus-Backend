//! Command run ViewModel — the single shape shared by `POST /admin/commands/execute`
//! and `GET /admin/commands` (history) (design §18.10, contract §22).

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, FromRow, utoipa::ToSchema)]
pub struct CommandRun {
    /// Stable run id (the `command_runs.id` primary key).
    pub command_id: Uuid,
    /// Raw PMP CLI text (empty for typed action runs).
    pub command: String,
    /// Action id (`pmp.cli.execute` for console runs).
    pub action: String,
    /// `queued` | `running` | `succeeded` | `failed` | `cancelled`.
    pub status: String,
    /// Result output (null when absent).
    pub output: Option<String>,
    /// Error message (null when absent).
    pub error: Option<String>,
    /// When the run finished.
    pub executed_at: Option<DateTime<Utc>>,
    /// Principal (user id) that initiated the run.
    pub principal: String,
    /// `personal` | `server`.
    pub scope: String,
}

/// Paginated command history response (§22 `{items, total, page, pageNum}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CommandRunListResponse {
    pub items: Vec<CommandRun>,
    pub total: i64,
    pub page: i64,
    #[serde(rename = "pageNum")]
    pub page_num: i64,
}
