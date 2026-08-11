//! Action Registry — all composable management actions (design §9.1).

use std::collections::HashMap;

use serde_json::Value;

use super::types::{ActionDescriptor, Executor, Risk};

/// Seed actions (Phase A). More are added in Phase B as typed commands land.
pub fn seed_actions() -> Vec<ActionDescriptor> {
    vec![
        ActionDescriptor::new(
            "room.kick",
            "room:kick",
            Executor::OpenUds,
            Risk::Medium,
            true,
            false,
            true, // host_allowed
            "room:{room_id}",
            false,
        ),
        ActionDescriptor::new(
            "room.set_chart",
            "room:config",
            Executor::OpenUds,
            Risk::Medium,
            true,
            false,
            true, // host_allowed
            "room:{room_id}",
            false,
        ),
        ActionDescriptor::new(
            "broadcast.all",
            "broadcast:all",
            Executor::OpenUds,
            Risk::High,
            true,
            false,
            false,
            "server",
            false,
        ),
        ActionDescriptor::new(
            "pmp.cli.execute",
            "pmp:cli",
            Executor::CliRaw,
            Risk::High,
            true,
            false,
            false,
            "server",
            false,
        ),
        ActionDescriptor::new(
            "pmp.update.apply",
            "server:update",
            Executor::CliExecute,
            Risk::Critical,
            true,
            true,
            false,
            "server",
            true, // long-running
        ),
    ]
}

/// Registry of actions keyed by id.
#[derive(Debug, Clone, Default)]
pub struct ActionRegistry {
    actions: HashMap<&'static str, &'static ActionDescriptor>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        for action in seed_actions() {
            // Leak the descriptors for a static lifetime (they are immutable seeds).
            let leaked: &'static ActionDescriptor = Box::leak(Box::new(action));
            registry.actions.insert(leaked.id, leaked);
        }
        registry
    }

    pub fn get(&self, id: &str) -> Option<&ActionDescriptor> {
        self.actions.get(id).copied()
    }

    pub fn all(&self) -> Vec<&'static ActionDescriptor> {
        self.actions.values().copied().collect()
    }

    /// Resolve the concrete queue key by substituting `{room_id}` (etc.) from args.
    pub fn resolve_queue_key(&self, action: &ActionDescriptor, args: &Value) -> String {
        if !action.queue_key.contains('{') {
            return action.queue_key.to_string();
        }
        let mut key = action.queue_key.to_string();
        if let Some(room_id) = args.get("room_id").and_then(Value::as_str) {
            key = key.replace("{room_id}", room_id);
        }
        key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_present() {
        let reg = ActionRegistry::new();
        assert!(reg.get("room.kick").is_some());
        assert!(reg.get("pmp.cli.execute").is_some());
        assert!(reg.get("pmp.update.apply").is_some());
        assert_eq!(reg.all().len(), 5);
    }

    #[test]
    fn queue_key_substitution() {
        let reg = ActionRegistry::new();
        let kick = reg.get("room.kick").unwrap();
        let key = reg.resolve_queue_key(kick, &serde_json::json!({ "room_id": "ABC" }));
        assert_eq!(key, "room:ABC");
        let missing = reg.resolve_queue_key(kick, &serde_json::json!({}));
        assert_eq!(missing, "room:{room_id}");
    }

    #[test]
    fn update_apply_is_reauth_long_running() {
        let reg = ActionRegistry::new();
        let a = reg.get("pmp.update.apply").unwrap();
        assert!(a.reauth);
        assert!(a.long_running);
        assert_eq!(a.permission, "server:update");
    }
}
