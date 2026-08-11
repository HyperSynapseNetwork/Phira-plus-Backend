//! Action executor wiring OpenUDS / cli.execute, with command_runs recording.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use super::registry::ActionRegistry;
use super::types::Executor;
use crate::commands::broker::{ActionExecutor as _ActionExecutorTrait, CommandTask};
use crate::commands::repo as command_repo;
use crate::pmp::cli as pmp_cli;
use crate::pmp::openuds::client::OpenUdsClient;

/// Executes action tasks against PMP (OpenUDS + CLI).
#[derive(Clone)]
pub struct PmpActionExecutor {
    openuds: Arc<OpenUdsClient>,
    registry: Arc<ActionRegistry>,
    db: Option<sqlx::PgPool>,
}

impl PmpActionExecutor {
    pub fn new(
        openuds: Arc<OpenUdsClient>,
        registry: Arc<ActionRegistry>,
        db: Option<sqlx::PgPool>,
    ) -> Self {
        Self {
            openuds,
            registry,
            db,
        }
    }

    async fn run(&self, task: &CommandTask) -> Result<Value, String> {
        let action = self
            .registry
            .get(&task.action)
            .ok_or_else(|| format!("unknown action: {}", task.action))?;

        match action.executor {
            Executor::OpenUds => {
                // Seed OpenUDS actions use the action id as the OpenUDS command name.
                self.openuds
                    .command(&task.action, task.args.clone())
                    .await
                    .map_err(|e| e.to_string())
            }
            Executor::CliExecute | Executor::CliRaw => {
                let cmd = task
                    .args
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if cmd.is_empty() {
                    return Err("cli.execute requires args.command".to_string());
                }
                pmp_cli::cli_execute(&self.openuds, &cmd)
                    .await
                    .map_err(|e| e.to_string())
            }
            Executor::Internal => Err("internal executor not implemented in Phase A".to_string()),
        }
    }
}

#[async_trait]
impl _ActionExecutorTrait for PmpActionExecutor {
    async fn execute(&self, task: CommandTask) -> Result<Value, String> {
        let command_id = task.command_id;
        let completion = task.completion;
        let result = self.run(&task).await;
        let (status, summary, error_code) = match &result {
            Ok(v) => ("succeeded", v.to_string(), String::new()),
            Err(e) => ("failed", String::new(), truncate(e, 120)),
        };
        if let Some(db) = &self.db {
            let _ = command_repo::mark_finished(db, command_id, status, &summary, &error_code).await;
        }
        if let Some(tx) = completion {
            let _ = tx.send(result.clone());
        }
        result
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
