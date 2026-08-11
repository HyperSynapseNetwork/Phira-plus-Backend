//! PMP events → PPB SSE envelope mapping + in-memory EventBus (with short replay).
//!
//! Contract §3 envelope: `{id, type, version, occurred_at, resource:{type,id}, data}`.
//! Mapped event types (never `broadcast.room` masquerading as player chat):
//! `user.online/offline`, `room.created/updated/joined/left`, `round.started/completed`,
//! `server.heartbeat`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;

use super::openuds::types::EventFrame;

/// Resource reference in the envelope.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceRef {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub id: String,
}

impl ResourceRef {
    pub fn room(id: &str) -> Self {
        Self { resource_type: "room".to_string(), id: id.to_string() }
    }
    pub fn user(id: &str) -> Self {
        Self { resource_type: "user".to_string(), id: id.to_string() }
    }
    pub fn server() -> Self {
        Self { resource_type: "server".to_string(), id: "main".to_string() }
    }
}

/// The external SSE event envelope.
#[derive(Debug, Clone, Serialize)]
pub struct PpbEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub version: u32,
    #[serde(rename = "occurred_at")]
    pub occurred_at: DateTime<Utc>,
    pub resource: ResourceRef,
    pub data: Value,
}

/// Result of trying to replay from a Last-Event-ID.
pub enum ReplayResult {
    /// Events after the given id.
    Events(Vec<Arc<PpbEvent>>),
    /// The id was not found; caller must fall back to snapshot + realtime.
    Miss,
}

/// In-memory event bus with a bounded history ring buffer for short replay.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<PpbEvent>>,
    history: Arc<Mutex<VecDeque<Arc<PpbEvent>>>>,
    max_history: usize,
}

#[allow(clippy::new_without_default)]
impl EventBus {
    pub fn new(capacity: usize, max_history: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self {
            tx,
            history: Arc::new(Mutex::new(VecDeque::new())),
            max_history,
        }
    }

    pub fn publish(&self, event: PpbEvent) {
        let arc = Arc::new(event);
        {
            let mut history = self.history.lock().unwrap();
            history.push_back(arc.clone());
            while history.len() > self.max_history {
                history.pop_front();
            }
        }
        let _ = self.tx.send(arc);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<PpbEvent>> {
        self.tx.subscribe()
    }

    /// Replay events strictly after `last_event_id` if it is still in the buffer.
    pub fn replay_from(&self, last_event_id: &str) -> ReplayResult {
        let history = self.history.lock().unwrap();
        let pos = history.iter().position(|e| e.id == last_event_id);
        match pos {
            Some(i) => ReplayResult::Events(history.iter().skip(i + 1).cloned().collect()),
            None => ReplayResult::Miss,
        }
    }

    /// Snapshot of the current history (fallback path).
    pub fn snapshot(&self) -> Vec<Arc<PpbEvent>> {
        self.history.lock().unwrap().iter().cloned().collect()
    }
}

/// Map a raw PMP OpenUDS event frame into the external PPB envelope.
/// Returns `None` for unmapped / non-public event types.
pub fn map_pmp_event(frame: &EventFrame) -> Option<PpbEvent> {
    let event_type = frame.event_type.clone();
    let occurred_at = chrono::DateTime::from_timestamp(frame.timestamp, 0).unwrap_or_else(Utc::now);

    let resource = match event_type.as_str() {
        "user.online" | "user.offline" => {
            let uid = frame.data.get("user_id").and_then(Value::as_i64).unwrap_or(0);
            ResourceRef::user(&uid.to_string())
        }
        "room.created" | "room.updated" | "room.joined" | "room.left" => {
            let rid = frame
                .data
                .get("room_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            ResourceRef::room(&rid)
        }
        "round.started" | "round.completed" => {
            let rid = frame
                .data
                .get("room_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            ResourceRef::room(&rid)
        }
        "server.heartbeat" => ResourceRef::server(),
        _ => return None,
    };

    Some(PpbEvent {
        id: frame.event_id.clone(),
        event_type,
        version: 1,
        occurred_at,
        resource,
        data: frame.data.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(event_type: &str, data: Value) -> EventFrame {
        EventFrame {
            frame_type: Some("event".to_string()),
            event_type: event_type.to_string(),
            data,
            event_id: uuid::Uuid::new_v4().to_string(),
            timestamp: 1_700_000_000,
        }
    }

    #[test]
    fn maps_known_types() {
        let cases = [
            ("user.online", json!({"user_id": 5, "name": "a"})),
            ("room.created", json!({"room_id": "ABC"})),
            ("room.joined", json!({"room_id": "ABC", "user_id": 5})),
            ("round.started", json!({"room_id": "ABC", "round_id": "r1"})),
            ("server.heartbeat", json!({"users": 1, "rooms": 2, "sessions": 3})),
        ];
        for (t, data) in cases {
            let mapped = map_pmp_event(&frame(t, data)).unwrap();
            assert_eq!(mapped.event_type, t);
            assert_eq!(mapped.version, 1);
        }
    }

    #[test]
    fn skips_unmapped_types() {
        // These must never leak to external clients.
        let bad = [
            "touches.received",
            "judges.received",
            "chat.message",
            "custom",
            "broadcast.room",
        ];
        for t in bad {
            assert!(map_pmp_event(&frame(t, json!({}))).is_none(), "{t} must be filtered");
        }
    }

    #[test]
    fn replay_and_snapshot() {
        let bus = EventBus::new(16, 8);
        let e1 = map_pmp_event(&frame("room.created", json!({"room_id": "A"}))).unwrap();
        let e2 = map_pmp_event(&frame("room.joined", json!({"room_id": "A", "user_id": 1}))).unwrap();
        bus.publish(e1.clone());
        bus.publish(e2.clone());

        match bus.replay_from(&e1.id) {
            ReplayResult::Events(events) => {
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].id, e2.id);
            }
            ReplayResult::Miss => panic!("should be present"),
        }
        match bus.replay_from("missing-id") {
            ReplayResult::Miss => {}
            ReplayResult::Events(_) => panic!("should miss"),
        }
        assert_eq!(bus.snapshot().len(), 2);
    }

    #[test]
    fn resource_types() {
        let ev = map_pmp_event(&frame("room.updated", json!({"room_id": "Z9"}))).unwrap();
        assert_eq!(ev.resource.resource_type, "room");
        assert_eq!(ev.resource.id, "Z9");
    }
}
