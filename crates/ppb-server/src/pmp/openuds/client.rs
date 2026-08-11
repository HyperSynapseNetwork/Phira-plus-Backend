//! OpenUDS client: connect, auth (token|approve), typed commands, subscribe,
//! subscribe_stream, events/streams, reconnect with exponential backoff + jitter.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rand::Rng;
use serde_json::{json, Value};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tokio::sync::{broadcast, oneshot, Notify, RwLock};

use super::protocol::{self, ProtocolError};
use super::types::{AuthPendingFrame, AuthenticatedFrame, EventFrame, ResponseFrame, StreamFrame};
use crate::config::PmpConfig;
use crate::error::{ApiError, ErrorCode};
use crate::pmp::capabilities::active_capabilities;

/// Authentication mode for OpenUDS.
#[derive(Debug, Clone)]
pub enum OpenUdsAuth {
    /// `{"type":"authenticate","token":...}`.
    Token(String),
    /// `{"type":"authenticate","client_name":...}` + operator `approve openuds <pending_id>`.
    Approve { client_name: String },
}

/// Client configuration.
#[derive(Debug, Clone)]
pub struct OpenUdsConfig {
    pub path: PathBuf,
    pub auth: OpenUdsAuth,
    pub reconnect_base_ms: u64,
    pub reconnect_max_ms: u64,
    pub request_timeout_ms: u64,
    pub capabilities: Vec<String>,
}

impl OpenUdsConfig {
    /// Build from runtime config + optional token secret.
    pub fn from_runtime(cfg: &PmpConfig, token: Option<String>) -> Self {
        let auth = match token {
            Some(t) if !t.is_empty() => OpenUdsAuth::Token(t),
            _ => OpenUdsAuth::Approve {
                client_name: cfg.client_name.clone(),
            },
        };
        Self {
            path: cfg.openuds_path.clone(),
            auth,
            reconnect_base_ms: cfg.reconnect_base_ms,
            reconnect_max_ms: cfg.reconnect_max_ms,
            request_timeout_ms: cfg.request_timeout_ms,
            capabilities: cfg.capabilities.clone(),
        }
    }
}

/// Connection state snapshot.
#[derive(Debug, Clone, Default)]
pub struct ConnectionState {
    pub connected: bool,
    pub session_id: Option<String>,
    pub server_version: Option<String>,
}

/// OpenUDS client (cloneable; shares a single connection + reconnect loop).
pub struct OpenUdsClient {
    pub config: Arc<OpenUdsConfig>,
    state: Arc<RwLock<ConnectionState>>,
    caps: Arc<RwLock<std::collections::HashSet<String>>>,
    attempts: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    shutdown_notify: Arc<Notify>,
    writer: Arc<tokio::sync::Mutex<Option<OwnedWriteHalf>>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<ResponseFrame>>>>,
    next_id: Arc<AtomicU64>,
    events_tx: broadcast::Sender<EventFrame>,
    streams_tx: broadcast::Sender<StreamFrame>,
}

impl std::fmt::Debug for OpenUdsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenUdsClient")
            .field("path", &self.config.path)
            .finish()
    }
}

impl OpenUdsClient {
    /// Create a client. Call `start()` to begin connecting.
    pub fn new(config: OpenUdsConfig) -> Self {
        let (events_tx, _) = broadcast::channel(1024);
        let (streams_tx, _) = broadcast::channel(256);
        Self {
            config: Arc::new(config),
            state: Arc::new(RwLock::new(ConnectionState::default())),
            caps: Arc::new(RwLock::new(std::collections::HashSet::new())),
            attempts: Arc::new(AtomicU64::new(0)),
            shutdown: Arc::new(AtomicBool::new(false)),
            shutdown_notify: Arc::new(Notify::new()),
            writer: Arc::new(tokio::sync::Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            events_tx,
            streams_tx,
        }
    }

    /// Spawn the background connect/reconnect loop.
    pub fn start(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run().await;
        });
    }

    /// Initiate graceful shutdown.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.shutdown_notify.notify_one();
    }

    async fn run(&self) {
        let mut attempt = 0u64;
        while !self.shutdown.load(Ordering::Acquire) {
            let result = self.connect_once().await;
            match result {
                Ok(()) => {
                    // Connection ended cleanly (EOF / reader loop exit).
                    tracing::debug!("openuds connection ended");
                    attempt = 0;
                }
                Err(e) => {
                    tracing::warn!(error = %e, attempt, "openuds connect failed");
                    attempt += 1;
                }
            }
            self.set_disconnected().await;
            if self.shutdown.load(Ordering::Acquire) {
                break;
            }
            let delay = next_backoff(attempt, self.config.reconnect_base_ms, self.config.reconnect_max_ms);
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = self.shutdown_notify.notified() => break,
            }
        }
    }

    async fn connect_once(&self) -> Result<(), OpenUdsError> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        let stream = UnixStream::connect(&self.config.path)
            .await
            .map_err(|e| OpenUdsError::Io(e.to_string()))?;
        let (mut reader, mut writer) = stream.into_split();

        let auth_frame = match &self.config.auth {
            OpenUdsAuth::Token(t) => json!({ "type": "authenticate", "token": t }),
            OpenUdsAuth::Approve { client_name } => {
                json!({ "type": "authenticate", "client_name": client_name })
            }
        };
        protocol::write_frame_async(&mut writer, &auth_frame)
            .await
            .map_err(|e| OpenUdsError::Io(e.to_string()))?;

        // First frame after auth attempt.
        let first = protocol::read_frame_async(&mut reader)
            .await
            .map_err(|e| OpenUdsError::Protocol(e.to_string()))?;

        match first.get("type").and_then(Value::as_str) {
            Some("authenticated") => {
                let auth: AuthenticatedFrame = serde_json::from_value(first)
                    .map_err(|e| OpenUdsError::Protocol(e.to_string()))?;
                self.set_authenticated(auth).await;
            }
            Some("auth_pending") => {
                let pending: AuthPendingFrame = serde_json::from_value(first)
                    .map_err(|e| OpenUdsError::Protocol(e.to_string()))?;
                tracing::warn!(
                    pending_id = %pending.pending_id,
                    "openuds approve-mode: run `approve openuds <pending_id>` (TTL 120s)"
                );
                // Wait for authenticated or auth_error (operator approves within 120s).
                let next = protocol::read_frame_async(&mut reader)
                    .await
                    .map_err(|e| OpenUdsError::Protocol(e.to_string()))?;
                match next.get("type").and_then(Value::as_str) {
                    Some("authenticated") => {
                        let auth: AuthenticatedFrame = serde_json::from_value(next)
                            .map_err(|e| OpenUdsError::Protocol(e.to_string()))?;
                        self.set_authenticated(auth).await;
                    }
                    Some("auth_error") => {
                        return Err(OpenUdsError::AuthRequired(
                            next.get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("approval rejected")
                                .to_string(),
                        ));
                    }
                    _ => return Err(OpenUdsError::Protocol("unexpected auth frame".into())),
                }
            }
            Some("auth_error") => {
                let msg = first
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("authentication failed")
                    .to_string();
                return Err(OpenUdsError::AuthRequired(msg));
            }
            _ => {
                return Err(OpenUdsError::Protocol("unexpected first frame".into()));
            }
        }

        // Install the writer half for command/subscribe.
        *self.writer.lock().await = Some(writer);

        // Reader loop until EOF/error.
        self.reader_loop(reader).await;
        Ok(())
    }

    async fn set_authenticated(&self, auth: AuthenticatedFrame) {
        let caps = active_capabilities(&self.config.capabilities, Some(&auth.server_version));
        *self.caps.write().await = caps;
        let mut st = self.state.write().await;
        st.connected = true;
        st.session_id = Some(auth.session_id);
        st.server_version = Some(auth.server_version);
        tracing::info!(
            version = ?st.server_version,
            "openuds connected"
        );
    }

    async fn set_disconnected(&self) {
        {
            let mut st = self.state.write().await;
            st.connected = false;
            st.session_id = None;
        }
        *self.writer.lock().await = None;
        // Drop pending command senders -> receivers observe Closed.
        self.pending.lock().unwrap().clear();
    }

    async fn reader_loop(&self, mut reader: OwnedReadHalf) {
        while let Ok(value) = protocol::read_frame_async(&mut reader).await {
            self.handle_frame(value).await;
        }
        self.set_disconnected().await;
    }

    async fn handle_frame(&self, value: Value) {
        let frame_type = value.get("type").and_then(Value::as_str).unwrap_or("");
        match frame_type {
            "response" => {
                if let Ok(frame) = serde_json::from_value::<ResponseFrame>(value) {
                    if let Some(id) = frame.id.clone() {
                        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                            let _ = tx.send(frame);
                        }
                    }
                }
            }
            "event" => {
                if let Ok(frame) = serde_json::from_value::<EventFrame>(value) {
                    let _ = self.events_tx.send(frame);
                }
            }
            "stream" => {
                if let Ok(frame) = serde_json::from_value::<StreamFrame>(value) {
                    let _ = self.streams_tx.send(frame);
                }
            }
            "subscribed" | "unsubscribed" | "stream_subscribed" | "pong" => {}
            _ => tracing::debug!(frame_type, "unhandled openuds frame"),
        }
    }

    // ── Public API ─────────────────────────────────────────────

    /// Send a command and await the typed response envelope.
    pub async fn command(&self, command: &str, params: Value) -> Result<Value, OpenUdsError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed).to_string();
        let frame = json!({
            "type": "command",
            "command": command,
            "params": params,
            "id": id,
        });
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id.clone(), tx);

        {
            let mut guard = self.writer.lock().await;
            match guard.as_mut() {
                Some(w) => {
                    if let Err(e) = protocol::write_frame_async(w, &frame).await {
                        self.pending.lock().unwrap().remove(&id);
                        return Err(OpenUdsError::Io(e.to_string()));
                    }
                }
                None => {
                    self.pending.lock().unwrap().remove(&id);
                    return Err(OpenUdsError::Unavailable("openuds not connected".into()));
                }
            }
        }

        let timeout = Duration::from_millis(self.config.request_timeout_ms);
        let resp = tokio::time::timeout(timeout, rx)
            .await
            .map_err(|_| {
                self.pending.lock().unwrap().remove(&id);
                OpenUdsError::Timeout(self.config.request_timeout_ms)
            })?
            .map_err(|_| OpenUdsError::Unavailable("openuds connection closed".into()))?;

        if resp.ok {
            Ok(resp.data.unwrap_or(Value::Null))
        } else {
            let err = resp.error.unwrap_or(super::types::ResponseError {
                code: "COMMAND_ERROR".to_string(),
                message: "command failed".to_string(),
            });
            Err(OpenUdsError::Command {
                command: command.to_string(),
                code: err.code,
                message: err.message,
            })
        }
    }

    /// Subscribe to event types (wildcards supported, e.g. `room.*`).
    pub async fn subscribe(&self, event_types: &[String]) -> Result<(), OpenUdsError> {
        let frame = json!({ "type": "subscribe", "event_types": event_types });
        self.send_raw(frame).await
    }

    /// Unsubscribe from event types.
    pub async fn unsubscribe(&self, event_types: &[String]) -> Result<(), OpenUdsError> {
        let frame = json!({ "type": "unsubscribe", "event_types": event_types });
        self.send_raw(frame).await
    }

    /// Subscribe to a high-frequency stream: touches | judges | logs.
    pub async fn subscribe_stream(&self, stream: &str) -> Result<(), OpenUdsError> {
        let frame = json!({ "type": "subscribe_stream", "stream": stream });
        self.send_raw(frame).await
    }

    /// Ping the server (keeps the socket alive in some deployments).
    pub async fn ping(&self) -> Result<(), OpenUdsError> {
        self.send_raw(json!({ "type": "ping" })).await
    }

    async fn send_raw(&self, frame: Value) -> Result<(), OpenUdsError> {
        let mut guard = self.writer.lock().await;
        match guard.as_mut() {
            Some(w) => protocol::write_frame_async(w, &frame)
                .await
                .map_err(|e| OpenUdsError::Io(e.to_string())),
            None => Err(OpenUdsError::Unavailable("openuds not connected".into())),
        }
    }

    /// Guard a capability; missing → `CAPABILITY_NOT_SUPPORTED`.
    pub async fn ensure_capability(&self, capability: &str) -> Result<(), OpenUdsError> {
        let caps = self.caps.read().await;
        if caps.contains(capability) {
            Ok(())
        } else {
            Err(OpenUdsError::CapabilityNotSupported(capability.to_string()))
        }
    }

    pub async fn state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    pub async fn capabilities(&self) -> Vec<String> {
        self.caps.read().await.iter().cloned().collect()
    }

    /// Total connection attempts (including reconnects).
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<EventFrame> {
        self.events_tx.subscribe()
    }

    pub fn subscribe_stream_frames(&self) -> broadcast::Receiver<StreamFrame> {
        self.streams_tx.subscribe()
    }
}

/// Exponential backoff with ±20% jitter, capped at `max_ms`.
pub fn next_backoff(attempt: u64, base_ms: u64, max_ms: u64) -> Duration {
    if attempt == 0 {
        return Duration::from_millis(base_ms.min(max_ms));
    }
    let exponent = (attempt - 1).min(10);
    let delay = base_ms.saturating_mul(2u64.saturating_pow(exponent as u32));
    let jitter = rand::thread_rng().gen_range(80..=120);
    let delay = delay.saturating_mul(jitter) / 100;
    Duration::from_millis(delay.min(max_ms))
}

/// OpenUDS errors.
#[derive(Debug, thiserror::Error)]
pub enum OpenUdsError {
    #[error("openuds unavailable: {0}")]
    Unavailable(String),
    #[error("command {command} failed: {code} {message}")]
    Command {
        command: String,
        code: String,
        message: String,
    },
    #[error("capability not supported: {0}")]
    CapabilityNotSupported(String),
    #[error("openuds request timed out after {0}ms")]
    Timeout(u64),
    #[error("openuds authentication required: {0}")]
    AuthRequired(String),
    #[error("openuds protocol error: {0}")]
    Protocol(String),
    #[error("openuds io error: {0}")]
    Io(String),
}

impl From<OpenUdsError> for ApiError {
    fn from(e: OpenUdsError) -> Self {
        match e {
            OpenUdsError::CapabilityNotSupported(cap) => ApiError::with_details(
                ErrorCode::CapabilityNotSupported,
                format!("capability not supported: {cap}"),
                json!({ "capability": cap }),
            ),
            OpenUdsError::Command { message, .. } => ApiError::new(ErrorCode::PmpUnavailable, message),
            OpenUdsError::Unavailable(m)
            | OpenUdsError::Protocol(m)
            | OpenUdsError::Io(m) => ApiError::new(ErrorCode::PmpUnavailable, m),
            OpenUdsError::Timeout(ms) => ApiError::new(
                ErrorCode::PmpUnavailable,
                format!("openuds request timed out after {ms}ms"),
            ),
            OpenUdsError::AuthRequired(m) => ApiError::new(ErrorCode::PmpUnavailable, m),
        }
    }
}

impl From<ProtocolError> for OpenUdsError {
    fn from(e: ProtocolError) -> Self {
        Self::Protocol(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_increases_and_caps() {
        // Deterministic bounds (jitter range 0.8x..1.2x).
        let d0 = next_backoff(0, 500, 30_000);
        assert_eq!(d0.as_millis() as u64, 500);

        let d1 = next_backoff(1, 500, 30_000);
        assert!((400..=600).contains(&d1.as_millis()));

        let d2 = next_backoff(2, 500, 30_000);
        assert!((800..=1200).contains(&d2.as_millis()));

        // Capped at max regardless of attempt.
        let dbig = next_backoff(50, 500, 30_000);
        assert!(dbig.as_millis() as u64 <= 30_000);
    }

    #[test]
    fn client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenUdsClient>();
    }
}
