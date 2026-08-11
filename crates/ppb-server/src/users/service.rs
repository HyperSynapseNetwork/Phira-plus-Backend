//! Player command service (PMP OpenUDS player.*) + admin user operations.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::pmp::openuds::client::{OpenUdsClient, OpenUdsError};

/// Typed wrapper over PMP player.* commands.
#[derive(Clone)]
pub struct PlayerService {
    openuds: Arc<OpenUdsClient>,
}

impl PlayerService {
    pub fn new(openuds: Arc<OpenUdsClient>) -> Self {
        Self { openuds }
    }

    pub async fn ban(&self, user_id: i32, reason: &str) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("player.ban", json!({ "user_id": user_id, "reason": reason }))
            .await
    }

    pub async fn unban(&self, user_id: i32) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("player.unban", json!({ "user_id": user_id }))
            .await
    }

    pub async fn banlist(&self) -> Result<Value, OpenUdsError> {
        self.openuds.command("player.banlist", json!({})).await
    }

    /// `target` may be an IP address or a user id.
    pub async fn ban_ip(&self, target: &str, reason: &str) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("player.ban_ip", json!({ "target": target, "reason": reason }))
            .await
    }

    pub async fn unban_ip(&self, ip: &str) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("player.unban_ip", json!({ "ip": ip }))
            .await
    }

    pub async fn ip_history(&self, user_id: i32) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("player.ip_history", json!({ "user_id": user_id }))
            .await
    }

    pub async fn info(&self, user_id: i32) -> Result<Value, OpenUdsError> {
        self.openuds
            .command("player.info", json!({ "user_id": user_id }))
            .await
    }

    pub async fn kick(&self, user_id: i32) -> Result<Value, OpenUdsError> {
        self.openuds.command("player.kick", json!({ "user_id": user_id })).await
    }
}
