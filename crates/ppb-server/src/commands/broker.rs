//! Command Broker — per-queue-key serial execution (design §9.2).
//!
//! Not a single global FIFO. Commands sharing a `queue_key` (e.g. `room:ABC`,
//! `server`, `user:123`) execute serially; different keys run in parallel.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode};

/// A queued command task.
#[derive(Debug)]
pub struct CommandTask {
    pub command_id: Uuid,
    pub action: String,
    pub actor: String,
    pub resource_key: String,
    pub args: Value,
    pub args_redacted: Value,
    /// Completion signal for callers awaiting synchronous execution.
    pub completion: Option<oneshot::Sender<Result<Value, String>>>,
}

/// Executor abstraction so the broker is decoupled from OpenUDS/DB specifics.
#[async_trait]
pub trait ActionExecutor: Send + Sync {
    async fn execute(&self, task: CommandTask) -> Result<Value, String>;
}

/// Serial per-key command broker.
#[derive(Clone)]
pub struct CommandBroker {
    workers: Arc<DashMap<String, mpsc::Sender<CommandTask>>>,
    executor: Arc<dyn ActionExecutor>,
}

#[allow(clippy::new_without_default)]
impl CommandBroker {
    pub fn new(executor: Arc<dyn ActionExecutor>) -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
            executor,
        }
    }

    /// Submit a task. If `completion` is provided, the caller can await the result.
    pub fn submit(&self, task: CommandTask) -> Result<(), ApiError> {
        let key = task.resource_key.clone();
        let tx = self.workers.entry(key.clone()).or_insert_with(|| {
            let (tx, rx) = mpsc::channel(64);
            let executor = Arc::clone(&self.executor);
            tokio::spawn(worker(key.clone(), rx, executor));
            tx
        });
        tx.try_send(task)
            .map_err(|_| ApiError::new(ErrorCode::LongJobAccepted, "command queue full"))
    }
}

async fn worker(key: String, mut rx: mpsc::Receiver<CommandTask>, executor: Arc<dyn ActionExecutor>) {
    while let Some(task) = rx.recv().await {
        let result = executor.execute(task).await;
        match &result {
            Ok(value) => {
                tracing::info!(queue_key = %key, result = %value, "command succeeded");
            }
            Err(e) => {
                tracing::warn!(queue_key = %key, error = %e, "command failed");
            }
        }
    }
}

/// Redact args for audit: only keep keys we consider safe (no passwords/tokens).
pub fn redact_args(args: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(obj) = args.as_object() {
        for (k, v) in obj {
            let key = k.to_ascii_lowercase();
            if key.contains("password") || key.contains("token") || key.contains("secret") {
                out.insert(k.clone(), Value::String("[REDACTED]".to_string()));
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_keys() {
        let args = serde_json::json!({
            "room_id": "ABC",
            "password": "hunter2",
            "refresh_token": "abc",
        });
        let redacted = redact_args(&args);
        assert_eq!(redacted["room_id"], "ABC");
        assert_eq!(redacted["password"], "[REDACTED]");
        assert_eq!(redacted["refresh_token"], "[REDACTED]");
    }
}
