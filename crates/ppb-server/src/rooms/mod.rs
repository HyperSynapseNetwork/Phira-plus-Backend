//! Rooms domain — facade over PMP OpenUDS room.* commands (Phase A scaffold).
//!
//! PMP is the source of truth for rooms; PPB holds no room mirror DB (design §11.4).

use serde_json::{json, Value};

use crate::pmp::openuds::client::{OpenUdsClient, OpenUdsError};

/// Room command facade.
#[derive(Clone)]
pub struct RoomService {
    openuds: std::sync::Arc<OpenUdsClient>,
}

impl RoomService {
    pub fn new(openuds: std::sync::Arc<OpenUdsClient>) -> Self {
        Self { openuds }
    }

    /// Send a room chat message as the resolved phira_id.
    /// Client never specifies a trusted user_id (design §13.3).
    pub async fn chat_send(
        &self,
        room_id: &str,
        resolved_phira_id: i64,
        content: &str,
    ) -> Result<Value, OpenUdsError> {
        self.openuds.ensure_capability("room.chat_send").await?;
        self.openuds
            .command(
                "room.chat_send",
                json!({
                    "room_id": room_id,
                    "user_id": resolved_phira_id,
                    "content": content,
                }),
            )
            .await
    }

    pub async fn room_info(&self, room_id: &str) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("room.info", json!({ "room_id": room_id }))
            .await
    }

    pub async fn room_list(&self, _filters: &Value) -> Result<Value, OpenUdsError> {
        // PMP room.list ignores filters (D4); PPB filters in memory in Phase B.
        self.openuds.command("room.list", json!({})).await
    }

    pub async fn kick(&self, room_id: &str, user_id: i64, _reason: &str) -> Result<Value, OpenUdsError> {
        // PMP room.kick ignores reason (D4); Panel re-broadcasts reason separately.
        self.openuds
            .command("room.kick", json!({ "room_id": room_id, "user_id": user_id }))
            .await
    }

    pub async fn set_chart(&self, room_id: &str, chart_id: &str) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("room.set_chart", json!({ "room_id": room_id, "chart_id": chart_id }))
            .await
    }
}
