//! Typed room command service (PMP OpenUDS room.*/player.* wrappers).
//!
//! PMP is the source of truth for rooms; PPB holds no room mirror DB (design §11.4).
//! Params mirror PMP dispatch.rs exactly (verified against source).

use std::sync::Arc;

use serde_json::{json, Value};

use crate::pmp::openuds::client::{OpenUdsClient, OpenUdsError};

/// Typed wrapper over PMP room.* and related commands.
#[derive(Clone)]
pub struct RoomService {
    openuds: Arc<OpenUdsClient>,
}

impl RoomService {
    pub fn new(openuds: Arc<OpenUdsClient>) -> Self {
        Self { openuds }
    }

    async fn cmd(&self, command: &str, params: Value) -> Result<Value, OpenUdsError> {
        self.openuds.command(command, params).await
    }

    /// Command with a per-command timeout override (`Some(0)` = unlimited).
    async fn cmd_timeout(
        &self,
        command: &str,
        params: Value,
        timeout_ms: Option<u64>,
    ) -> Result<Value, OpenUdsError> {
        self.openuds.command_with_timeout(command, params, timeout_ms).await
    }

    // ── Room lifecycle ───────────────────────────────────────────

    pub async fn create(
        &self,
        room_id: &str,
        endpoint: Option<&str>,
        persistent_empty: bool,
    ) -> Result<Value, OpenUdsError> {
        let mut params = json!({ "room_id": room_id, "persistent_empty": persistent_empty });
        if let Some(ep) = endpoint {
            params["endpoint"] = json!(ep);
        }
        self.cmd("room.create", params).await
    }

    pub async fn close(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.close", json!({ "room_id": room_id })).await
    }

    pub async fn start(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.start", json!({ "room_id": room_id })).await
    }

    pub async fn cancel_start(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.cancel_start", json!({ "room_id": room_id })).await
    }

    pub async fn ready(&self, room_id: &str, user_id: i32) -> Result<Value, OpenUdsError> {
        self.cmd("room.ready", json!({ "room_id": room_id, "user_id": user_id })).await
    }

    pub async fn lock(&self, room_id: &str, locked: bool) -> Result<Value, OpenUdsError> {
        self.cmd("room.lock", json!({ "room_id": room_id, "locked": locked })).await
    }

    pub async fn cycle(&self, room_id: &str, cycle: bool) -> Result<Value, OpenUdsError> {
        self.cmd("room.cycle", json!({ "room_id": room_id, "cycle": cycle })).await
    }

    pub async fn set_host(&self, room_id: &str, host_id: Option<i32>) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_host", json!({ "room_id": room_id, "host_id": host_id })).await
    }

    pub async fn set_live(&self, room_id: &str, live: bool) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_live", json!({ "room_id": room_id, "live": live })).await
    }

    pub async fn set_chart(&self, room_id: &str, chart_id: i32) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_chart", json!({ "room_id": room_id, "chart_id": chart_id })).await
    }

    pub async fn set_hidden(&self, room_id: &str, hidden: bool) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_hidden", json!({ "room_id": room_id, "hidden": hidden })).await
    }

    pub async fn set_persistent(&self, room_id: &str, persistent: bool) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_persistent", json!({ "room_id": room_id, "persistent": persistent })).await
    }

    pub async fn set_degraded(&self, room_id: &str, degraded: bool) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_degraded", json!({ "room_id": room_id, "degraded": degraded })).await
    }

    pub async fn set_api_endpoint(&self, room_id: &str, endpoint: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.set_api_endpoint", json!({ "room_id": room_id, "endpoint": endpoint })).await
    }

    pub async fn kick(&self, room_id: &str, user_id: i32) -> Result<Value, OpenUdsError> {
        // PMP room.kick ignores reason (D4); reason is re-broadcast by Panel.
        self.cmd("room.kick", json!({ "room_id": room_id, "user_id": user_id })).await
    }

    pub async fn force_move(
        &self,
        room_id: &str,
        user_id: i32,
        monitor: bool,
    ) -> Result<Value, OpenUdsError> {
        self.cmd("room.force_move", json!({ "room_id": room_id, "user_id": user_id, "monitor": monitor })).await
    }

    // ── Room queries ─────────────────────────────────────────────

    pub async fn info(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.info", json!({ "room_id": room_id })).await
    }

    /// Current host phira_id from room.info (used to re-check host at execution).
    pub async fn host_id(&self, room_id: &str) -> Result<Option<i32>, OpenUdsError> {
        let info = self.info(room_id).await?;
        let host = info
            .get("host_id")
            .and_then(Value::as_i64)
            .map(|v| v as i32)
            .or_else(|| info.get("host").and_then(Value::as_i64).map(|v| v as i32));
        Ok(host)
    }

    /// `room.list` flattened into `(rooms, total)`.
    ///
    /// PMP returns `{rooms: [...], total: N}`; a bare array and `{results: [...]}`
    /// are tolerated as legacy passthrough shapes. `total` falls back to the
    /// array length when PMP omits it.
    pub async fn list(&self) -> Result<(Vec<Value>, i64), OpenUdsError> {
        let result = self.cmd("room.list", json!({})).await?;
        let rooms = room_array(&result);
        let total = result
            .get("total")
            .and_then(Value::as_i64)
            .unwrap_or(rooms.len() as i64);
        Ok((rooms, total))
    }

    pub async fn history(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        // room.history aggregates rounds + scores and can exceed the 10s default.
        self.cmd_timeout("room.history", json!({ "room_id": room_id }), Some(60_000)).await
    }

    pub async fn chat_history(&self, room_id: &str, limit: Option<u64>) -> Result<Value, OpenUdsError> {
        let mut params = json!({ "room_id": room_id });
        if let Some(l) = limit {
            params["limit"] = json!(l);
        }
        self.cmd("room.chat_history", params).await
    }

    /// Send a room chat message as `resolved_phira_id`. Client never supplies a
    /// trusted user_id (design §13.3); PMP validates the sender is a real player.
    pub async fn chat_send(
        &self,
        room_id: &str,
        resolved_phira_id: i32,
        content: &str,
    ) -> Result<Value, OpenUdsError> {
        self.openuds.ensure_capability("room.chat_send").await?;
        self.cmd(
            "room.chat_send",
            json!({ "room_id": room_id, "user_id": resolved_phira_id, "content": content }),
        )
        .await
    }

    pub async fn uuid(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.uuid", json!({ "room_id": room_id })).await
    }

    pub async fn rounds(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.rounds", json!({ "room_id": room_id })).await
    }

    pub async fn round(&self, room_id: &str, round_uuid: Option<&str>) -> Result<Value, OpenUdsError> {
        let mut params = json!({ "room_id": room_id });
        if let Some(r) = round_uuid {
            params["round_uuid"] = json!(r);
        }
        self.cmd("room.round", params).await
    }

    // ── Room ban/whitelist ───────────────────────────────────────

    pub async fn ban(&self, room_id: &str, user_id: i32) -> Result<Value, OpenUdsError> {
        self.cmd("room.ban", json!({ "room_id": room_id, "user_id": user_id })).await
    }

    pub async fn unban(&self, room_id: &str, user_id: i32) -> Result<Value, OpenUdsError> {
        self.cmd("room.unban", json!({ "room_id": room_id, "user_id": user_id })).await
    }

    pub async fn banlist(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.banlist", json!({ "room_id": room_id })).await
    }

    pub async fn whitelist(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.cmd("room.whitelist", json!({ "room_id": room_id })).await
    }

    pub async fn whitelist_add(&self, room_id: &str, user_id: i32) -> Result<Value, OpenUdsError> {
        self.cmd("room.whitelist_add", json!({ "room_id": room_id, "user_id": user_id })).await
    }

    pub async fn whitelist_remove(&self, room_id: &str, user_id: i32) -> Result<Value, OpenUdsError> {
        self.cmd("room.whitelist_remove", json!({ "room_id": room_id, "user_id": user_id })).await
    }
}

/// Extract the room array from a `room.list` payload: a bare array,
/// `{rooms: [...]}` or `{results: [...]}`. Empty when none matches.
fn room_array(result: &Value) -> Vec<Value> {
    if let Some(arr) = result.as_array() {
        return arr.clone();
    }
    for key in ["rooms", "results"] {
        if let Some(arr) = result.get(key).and_then(Value::as_array) {
            return arr.clone();
        }
    }
    Vec::new()
}
