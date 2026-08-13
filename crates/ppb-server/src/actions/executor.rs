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
                // Contract §18: room.kick/force_move/ban/whitelist target the player
                // via `args.phira_id`; PMP expects `user_id`. Normalize before send.
                let mut args = task.args.clone();
                normalize_openuds_args(&task.action, &mut args);
                self.openuds
                    .command(&task.action, args)
                    .await
                    .map_err(|e| e.to_string())
            }
            Executor::CliExecute => {
                // Stop-ship: `CliExecute` actions are server-fixed commands.
                // Clients must NOT supply CLI text — only `pmp.cli.execute`
                // (Executor::CliRaw) accepts arbitrary input.
                let cmd = fixed_cli_command(&task.action, &task.args)
                    .ok_or_else(|| format!("no fixed CLI mapping for action {}", task.action))?;
                pmp_cli::cli_execute(&self.openuds, &cmd)
                    .await
                    .map_err(|e| e.to_string())
            }
            Executor::CliRaw => {
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
        let result = self.run(&task).await;
        let command_id = task.command_id;
        let completion = task.completion;
        let (status, summary, error_code) = match &result {
            Ok(v) => ("succeeded", v.to_string(), String::new()),
            Err(e) => ("failed", String::new(), truncate(e, 120)),
        };
        if let Some(db) = &self.db {
            let _ = command_repo::mark_finished(db, command_id, status, &summary, &error_code).await;
            // Gate 0 A5: audited actions record the FINAL result after
            // execution completes — never a pre-recorded success.
            if let Some(audit) = &task.audit {
                let _ = crate::audit::service::record_completed_command(
                    db,
                    &audit.principal_type,
                    audit.actor_user_id,
                    audit.actor_session_id,
                    &audit.action,
                    &audit.resource_type,
                    &audit.resource_id,
                    task.args_redacted.clone(),
                    status,
                    &error_code,
                    &command_id.to_string(),
                    &audit.request_id,
                    &audit.ip,
                    &audit.user_agent,
                )
                .await;
            }
        }
        if let Some(tx) = completion {
            let _ = tx.send(result.clone());
        }
        result
    }
}

/// Server-fixed CLI mapping for `Executor::CliExecute` actions. Clients never
/// supply the command text — only `pmp.cli.execute` (CliRaw) is arbitrary.
///
/// `pmp.update.*` is deliberately absent: those long-running updates run only
/// through the Job API (`POST /admin/jobs`), not the generic Action executor.
/// `execute_action` rejects them up front; this mapping returns `None` so even
/// a broker-submitted update can never `cli.execute` directly.
fn fixed_cli_command(action: &str, args: &Value) -> Option<String> {
    match action {
        "server.connections" => match args.get("enabled").and_then(Value::as_bool) {
            Some(true) => Some("connections on".to_string()),
            Some(false) => Some("connections off".to_string()),
            // Absent `enabled` = read current state.
            None => Some("connections".to_string()),
        },
        _ => None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

/// Player-target commands accept `args.phira_id` (contract §18) and pass it to
/// PMP as `user_id`. room.set_chart already uses `args.chart_id` (PMP param).
fn normalize_openuds_args(action: &str, args: &mut Value) {
    const PLAYER_TARGET: &[&str] = &[
        "room.kick",
        "room.force_move",
        "room.ban",
        "room.unban",
        "room.whitelist_add",
        "room.whitelist_remove",
    ];
    if !PLAYER_TARGET.contains(&action) {
        return;
    }
    if let Some(phira_id) = args.get("phira_id").and_then(Value::as_i64) {
        if args.get("user_id").is_none() {
            if let Value::Object(map) = args {
                map.insert("user_id".to_string(), serde_json::json!(phira_id));
                map.remove("phira_id");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_phira_id_to_user_id_for_player_target() {
        let mut args = serde_json::json!({ "room_id": "ABC", "phira_id": 42 });
        normalize_openuds_args("room.kick", &mut args);
        assert_eq!(args["user_id"], 42);
        assert!(args.get("phira_id").is_none());
    }

    #[test]
    fn keeps_existing_user_id() {
        let mut args = serde_json::json!({ "room_id": "ABC", "phira_id": 42, "user_id": 7 });
        normalize_openuds_args("room.ban", &mut args);
        assert_eq!(args["user_id"], 7);
        assert!(args.get("phira_id").is_some(), "should not clobber explicit user_id");
    }

    #[test]
    fn set_chart_passes_chart_id_through() {
        let mut args = serde_json::json!({ "room_id": "ABC", "chart_id": 99 });
        normalize_openuds_args("room.set_chart", &mut args);
        assert_eq!(args["chart_id"], 99);
    }

    #[test]
    fn fixed_cli_commands_are_server_mapped() {
        assert_eq!(
            fixed_cli_command("server.connections", &serde_json::json!({ "enabled": true })),
            Some("connections on".to_string())
        );
        assert_eq!(
            fixed_cli_command("server.connections", &serde_json::json!({ "enabled": false })),
            Some("connections off".to_string())
        );
        assert_eq!(
            fixed_cli_command("server.connections", &serde_json::json!({})),
            Some("connections".to_string())
        );
        assert_eq!(fixed_cli_command("room.kick", &serde_json::json!({})), None);
    }

    #[test]
    fn update_actions_have_no_direct_cli_mapping() {
        // §9.4: `pmp.update.*` runs only through the Job API. There must be no
        // fixed CLI mapping for the generic Action executor.
        for id in ["pmp.update.check", "pmp.update.apply", "pmp.update.cancel", "pmp.update.force"] {
            assert_eq!(fixed_cli_command(id, &serde_json::json!({})), None, "{id} must not cli.execute");
        }
    }
}
