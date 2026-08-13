//! Typed OpenUDS frames.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A `{"type":"response",...}` frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFrame {
    #[serde(rename = "type", default)]
    pub frame_type: Option<String>,
    pub id: Option<String>,
    pub ok: bool,
    pub data: Option<Value>,
    pub error: Option<ResponseError>,
}

/// Error payload of a failed response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: String,
    pub message: String,
}

/// A `{"type":"event",...}` frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    #[serde(rename = "type", default)]
    pub frame_type: Option<String>,
    #[serde(rename = "event_type")]
    pub event_type: String,
    pub data: Value,
    #[serde(rename = "event_id")]
    pub event_id: String,
    pub timestamp: i64,
}

/// A `{"type":"stream",...}` frame (touches/judges/logs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamFrame {
    #[serde(rename = "type", default)]
    pub frame_type: Option<String>,
    pub stream: String,
    #[serde(rename = "user_id")]
    pub user_id: i64,
    pub frames: Value,
    pub sequence: Option<u64>,
    pub room: Option<String>,
    pub round: Option<String>,
    pub timestamp: Option<i64>,
}

/// The `{"type":"authenticated",...}` frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatedFrame {
    #[serde(rename = "type", default)]
    pub frame_type: Option<String>,
    #[serde(rename = "session_id")]
    pub session_id: String,
    #[serde(rename = "server_version")]
    pub server_version: String,
}

/// The `{"type":"auth_error",...}` frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthErrorFrame {
    #[serde(rename = "type", default)]
    pub frame_type: Option<String>,
    pub message: String,
}

/// Subscription acknowledgement frames.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckFrame {
    #[serde(rename = "type", default)]
    pub frame_type: Option<String>,
    pub active: Option<Vec<String>>,
    pub removed: Option<Vec<String>>,
    pub stream: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_response_success() {
        let v = json!({"type":"response","id":"req-1","ok":true,"data":{"ok":true}});
        let frame: ResponseFrame = serde_json::from_value(v).unwrap();
        assert!(frame.ok);
        assert_eq!(frame.id.as_deref(), Some("req-1"));
        assert!(frame.error.is_none());
    }

    #[test]
    fn parse_response_error() {
        let v = json!({"type":"response","id":"req-2","ok":false,"error":{"code":"UNKNOWN_COMMAND","message":"nope"}});
        let frame: ResponseFrame = serde_json::from_value(v).unwrap();
        assert!(!frame.ok);
        let err = frame.error.unwrap();
        assert_eq!(err.code, "UNKNOWN_COMMAND");
    }

    #[test]
    fn parse_event() {
        let v = json!({"type":"event","event_type":"room.created","data":{"room_id":"ABC"},"event_id":"e1","timestamp":123});
        let frame: EventFrame = serde_json::from_value(v).unwrap();
        assert_eq!(frame.event_type, "room.created");
        assert_eq!(frame.data["room_id"], "ABC");
    }

    #[test]
    fn parse_stream() {
        let v = json!({"type":"stream","stream":"touches","user_id":1001,"frames":[{"x":1}],"sequence":3,"room":"r","round":null,"timestamp":9});
        let frame: StreamFrame = serde_json::from_value(v).unwrap();
        assert_eq!(frame.stream, "touches");
        assert_eq!(frame.user_id, 1001);
        assert_eq!(frame.sequence, Some(3));
        assert!(frame.round.is_none());
    }

    #[test]
    fn parse_authenticated() {
        let v = json!({"type":"authenticated","session_id":"s1","server_version":"1.0.38"});
        let frame: AuthenticatedFrame = serde_json::from_value(v).unwrap();
        assert_eq!(frame.session_id, "s1");
        assert_eq!(frame.server_version, "1.0.38");
    }
}
