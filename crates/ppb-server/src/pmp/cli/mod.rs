//! `cli.execute` wrapper (design §9.3).
//!
//! `cli.execute` is a first-class capability. Typed OpenUDS commands take
//! priority; CLI is used for operations without a typed command and for the
//! raw Panel Console (full audit).

use serde_json::Value;

use super::openuds::client::{OpenUdsClient, OpenUdsError};

/// Execute a raw PMP CLI command through OpenUDS.
pub async fn cli_execute(openuds: &OpenUdsClient, command: &str) -> Result<Value, OpenUdsError> {
    openuds
        .command("cli.execute", serde_json::json!({ "command": command }))
        .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn command_payload_shape() {
        let payload = serde_json::json!({ "command": "update check" });
        assert_eq!(payload["command"], "update check");
    }
}
