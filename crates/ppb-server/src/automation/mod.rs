//! Automation / Runbook domain (design §10). Phase A: schema + safety invariants.

pub mod routes;
//!
//! V1 rules enforced later in Phase B:
//! - No arbitrary `/bin/bash` / PowerShell / host commands (no shell executor).
//! - Each step re-authorizes with the current principal.
//! - Runs store a `definition_snapshot` for audit.
//! - No IF/loop/complex expressions / scheduled triggers in V1.

use serde::{Deserialize, Serialize};

/// A runbook definition (stored as JSONB; snapshot on every run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunbookDefinition {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub args: serde_json::Value,
    pub steps: Vec<RunbookStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunbookStep {
    pub action: String,
    #[serde(default)]
    pub with: serde_json::Value,
    #[serde(default)]
    pub wait_secs: Option<u64>,
}

/// The set of actions a Runbook may reference (must already be in the Action Registry).
#[allow(dead_code)]
pub fn validate_steps(steps: &[RunbookStep], registry: &crate::actions::registry::ActionRegistry) -> Result<(), String> {
    for step in steps {
        if step.action.is_empty() {
            return Err("step action is empty".to_string());
        }
        if registry.get(&step.action).is_none() {
            return Err(format!("unknown action in runbook step: {}", step.action));
        }
        if step.wait_secs.unwrap_or(0) > 3600 {
            return Err("wait exceeds 3600s limit".to_string());
        }
    }
    Ok(())
}
