//! Action Registry — all composable management actions (design §9.1).

use std::collections::HashMap;

use serde_json::Value;

use super::types::{ActionDescriptor, Executor, Risk};

/// Seed actions covering the Phase B control set (design §9.1, §18.3/§18.6/§18.9).
///
/// Executor::OpenUds actions use the action id directly as the OpenUDS command
/// name; args are the command params (room_id/user_id etc.). `host_allowed`
/// actions re-derive the real host via `room.info` at execution time.
pub fn seed_actions() -> Vec<ActionDescriptor> {
    vec![
        // ── Room lifecycle (design §18.3) ─────────────────────────
        ActionDescriptor::new("room.kick", "room:kick", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.force_move", "room:move", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.start", "room:start", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.cancel_start", "room:start", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.close", "room:manage", Executor::OpenUds, Risk::High, true, false, false, "room:{room_id}", false),
        // ── Room config (host_allowed) ────────────────────────────
        ActionDescriptor::new("room.set_chart", "room:config", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.lock", "room:config", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.cycle", "room:config", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.set_live", "room:config", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.set_hidden", "room:config", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.set_persistent", "room:config", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        // `room.set_api_endpoint` is server/room-level configuration; a room
        // host must not be able to point the room at an arbitrary API endpoint.
        // Admin-only (not host_allowed).
        ActionDescriptor::new("room.set_api_endpoint", "room:config", Executor::OpenUds, Risk::High, true, false, false, "room:{room_id}", false),
        // Changing host / degraded state is admin-gated (not host_allowed).
        ActionDescriptor::new("room.set_host", "room:config", Executor::OpenUds, Risk::High, true, false, false, "room:{room_id}", false),
        ActionDescriptor::new("room.set_degraded", "room:config", Executor::OpenUds, Risk::High, true, false, false, "room:{room_id}", false),
        // ── Room lists (host_allowed) ─────────────────────────────
        ActionDescriptor::new("room.ban", "room:blacklist", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.unban", "room:blacklist", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.whitelist_add", "room:whitelist", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        ActionDescriptor::new("room.whitelist_remove", "room:whitelist", Executor::OpenUds, Risk::Medium, true, false, true, "room:{room_id}", false),
        // ── Player control (design §18.4 Security) ────────────────
        // §23 #10 Sensitive Action Policy: ban / IP ban always require reauth.
        ActionDescriptor::new("player.kick", "user:kick", Executor::OpenUds, Risk::Medium, true, false, false, "user:{user_id}", false),
        ActionDescriptor::new("player.ban", "user:ban", Executor::OpenUds, Risk::High, true, true, false, "user:{user_id}", false),
        ActionDescriptor::new("player.unban", "user:ban", Executor::OpenUds, Risk::Medium, true, true, false, "user:{user_id}", false),
        ActionDescriptor::new("player.ban_ip", "user:ban_ip", Executor::OpenUds, Risk::High, true, true, false, "server", false),
        ActionDescriptor::new("player.unban_ip", "user:ban_ip", Executor::OpenUds, Risk::High, true, true, false, "server", false),
        // ── Broadcast (design §18.9) ──────────────────────────────
        ActionDescriptor::new("broadcast.all", "broadcast:all", Executor::OpenUds, Risk::High, true, false, false, "server", false),
        ActionDescriptor::new("broadcast.room", "broadcast:room", Executor::OpenUds, Risk::Medium, true, false, false, "room:{room_id}", false),
        ActionDescriptor::new("broadcast.user", "broadcast:user", Executor::OpenUds, Risk::Medium, true, false, false, "server", false),
        // ── Server ops (design §18.6) ─────────────────────────────
        ActionDescriptor::new("server.config_reload", "config:reload", Executor::OpenUds, Risk::High, true, false, false, "config:pmp", false),
        ActionDescriptor::new("server.roomcreation", "server:manage", Executor::OpenUds, Risk::High, true, false, false, "server", false),
        ActionDescriptor::new("server.shutdown", "server:shutdown", Executor::OpenUds, Risk::Critical, true, true, false, "server", false),
        // ── PMP CLI / update (design §9.3) ────────────────────────
        // Raw Console requires reauth (same semantics as Automation/Runbook, §22).
        ActionDescriptor::new("pmp.cli.execute", "pmp:cli", Executor::CliRaw, Risk::High, true, true, false, "server", false),
        ActionDescriptor::new("pmp.update.apply", "server:update", Executor::CliExecute, Risk::Critical, true, true, false, "server", true),
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
        if let Some(user_id) = args.get("user_id") {
            key = key.replace("{user_id}", &user_id.to_string());
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
        assert!(reg.get("room.lock").is_some());
        assert!(reg.get("room.set_host").is_some());
        assert!(reg.get("player.ban").is_some());
        assert!(reg.get("broadcast.all").is_some());
        assert!(reg.get("server.config_reload").is_some());
        assert!(reg.get("pmp.cli.execute").is_some());
        assert!(reg.get("pmp.update.apply").is_some());
        assert!(reg.get("player.ban_ip").is_some());
        assert!(reg.all().len() >= 25);
    }

    #[test]
    fn room_lock_unified_no_unlock() {
        // §22: no room.unlock; lock/unlock both go through room.lock {locked:bool}.
        let reg = ActionRegistry::new();
        assert!(reg.get("room.lock").is_some());
        assert!(reg.get("room.unlock").is_none());
    }

    #[test]
    fn canonical_action_ids_present() {
        // §23 #2: Registry is the single source of Action IDs. PPF/Panel must
        // use these exact IDs; the legacy `user.*` / `room.blacklist_*` names
        // are NOT valid.
        let reg = ActionRegistry::new();
        for id in [
            "player.ban", "player.unban", "player.kick", "player.ban_ip", "player.unban_ip",
            "room.ban", "room.unban", "room.lock",
            "server.config_reload", "server.roomcreation", "server.shutdown",
            "pmp.cli.execute", "pmp.update.apply",
        ] {
            assert!(reg.get(id).is_some(), "missing canonical action {id}");
        }
        for id in ["user.ban", "user.unban", "user.kick", "user.ban_ip", "user.unban_ip",
                   "room.blacklist_ban", "room.blacklist_unban", "room.unlock"] {
            assert!(reg.get(id).is_none(), "legacy action {id} must not exist");
        }
    }

    #[test]
    fn sensitive_actions_require_reauth() {
        // §23 #10: ban / IP ban / shutdown / update / raw CLI require reauth.
        let reg = ActionRegistry::new();
        for id in [
            "player.ban", "player.unban", "player.ban_ip", "player.unban_ip",
            "server.shutdown", "pmp.cli.execute", "pmp.update.apply",
        ] {
            let a = reg.get(id).unwrap();
            assert!(a.reauth, "sensitive action {id} must require reauth");
        }
    }

    #[test]
    fn host_allowed_actions_are_room_scoped_and_audited() {
        // Gate 2: a room host must never reach server-level / global config.
        let reg = ActionRegistry::new();
        let mut found = 0;
        for a in reg.all() {
            if a.host_allowed {
                found += 1;
                assert!(a.audit, "host_allowed {} must be audited", a.id);
                assert!(
                    a.queue_key.starts_with("room:"),
                    "host_allowed {} must be room-scoped (got {})",
                    a.id,
                    a.queue_key
                );
            }
        }
        assert!(found > 0, "expected at least one host_allowed action");
    }

    #[test]
    fn set_api_endpoint_is_not_host_allowed() {
        // Gate 0: a room host must not re-point the room's API endpoint
        // (server/room-level config) — admin-only action.
        let reg = ActionRegistry::new();
        let a = reg.get("room.set_api_endpoint").unwrap();
        assert!(!a.host_allowed, "room.set_api_endpoint must not be host_allowed");
        assert!(a.audit, "room.set_api_endpoint must remain audited");
    }

    #[test]
    fn queue_key_substitution() {
        let reg = ActionRegistry::new();
        let kick = reg.get("room.kick").unwrap();
        let key = reg.resolve_queue_key(kick, &serde_json::json!({ "room_id": "ABC" }));
        assert_eq!(key, "room:ABC");
        let missing = reg.resolve_queue_key(kick, &serde_json::json!({}));
        assert_eq!(missing, "room:{room_id}");
        let pban = reg.get("player.ban").unwrap();
        assert_eq!(
            reg.resolve_queue_key(pban, &serde_json::json!({ "user_id": 42 })),
            "user:42"
        );
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
