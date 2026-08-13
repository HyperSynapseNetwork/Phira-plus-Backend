//! Logs domain.
//!
//! PMP provides `logs.history` / `logs.input` and the `logs` OpenUDS stream.
//! PPB keeps its own structured logs; the PMP Console path is via Action
//! Registry (`pmp.cli.execute`) and `/admin/logs/input`.

pub mod routes;
pub mod translator;

use serde_json::Value;

/// Shape of a `logs` stream frame's `frames` payload.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LogLine {
    pub line: String,
}

/// Structured log entry (contract §23 #3 / Panel §18.11).
///
/// PMP is the logs source of truth (§13); PPB does not hard-code PMP's internal
/// payload — each raw line is best-effort structured into this stable shape.
/// `service` is `"pmp"` for entries sourced from `logs.history`.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct LogEntry {
    pub log_id: String,
    pub timestamp: String,
    pub service: String,
    pub level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// Convert a raw `logs` stream frame payload into log lines (best-effort).
pub fn parse_log_frames(frames: &Value) -> Vec<String> {
    match frames {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.get("line").and_then(Value::as_str).map(str::to_string))
            })
            .collect(),
        Value::Object(map) => map
            .get("line")
            .and_then(Value::as_str)
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Extract log lines from a `logs.history` response (`{ lines: [...], count }`)
/// with a fallback to the generic stream-frame shapes.
pub fn history_lines_of(value: &Value) -> Vec<String> {
    match value.get("lines").and_then(Value::as_array) {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        None => parse_log_frames(value),
    }
}

/// Best-effort: structure one raw PMP log line. The full original line is always
/// preserved as `message`; `timestamp`/`level` are extracted when present.
/// `log_id` is a stable content hash so a line can be located across requests.
pub fn parse_log_line(line: &str) -> LogEntry {
    let trimmed = line.trim();
    let mut timestamp = String::new();
    let mut level = "info".to_string();
    let mut message = trimmed.to_string();

    if let Some(rest) = trimmed.strip_prefix('[') {
        // Bracketed form: `[2026-08-13T10:00:00Z] [INFO] message`.
        if let Some(close) = rest.find(']') {
            timestamp = rest[..close].to_string();
            let after = rest[close + 1..].trim_start();
            if let Some(after2) = after.strip_prefix('[') {
                if let Some(close2) = after2.find(']') {
                    level = normalize_level(&after2[..close2].to_ascii_lowercase());
                    message = after2[close2 + 1..].trim_start().to_string();
                }
            }
        }
    } else {
        // Unbracketed tracing fmt: `2026-08-13T10:00:00.123Z  INFO target: msg`.
        let mut parts = trimmed.splitn(3, char::is_whitespace);
        let first = parts.next().unwrap_or("");
        if looks_like_timestamp(first) {
            timestamp = first.to_string();
            let second = parts.next().unwrap_or("").trim_matches(|c: char| c == '[' || c == ']');
            let lvl = second.to_ascii_lowercase();
            if is_known_level(&lvl) {
                level = normalize_level(&lvl);
                message = parts.next().unwrap_or("").trim_start().to_string();
            } else {
                message = trimmed[first.len()..].trim_start().to_string();
            }
        }
    }

    if message.is_empty() {
        message = trimmed.to_string();
    }

    LogEntry {
        log_id: log_id_for(trimmed),
        timestamp,
        service: "pmp".to_string(),
        level,
        event: None,
        message,
        error_code: None,
        request_id: None,
        command_id: None,
        room_uuid: None,
        user_id: None,
    }
}

fn looks_like_timestamp(s: &str) -> bool {
    s.len() >= 10 && s.as_bytes()[4] == b'-' && s[..4].bytes().all(|b| b.is_ascii_digit())
}

fn is_known_level(s: &str) -> bool {
    matches!(s, "trace" | "debug" | "info" | "warn" | "warning" | "error" | "fatal")
}

fn normalize_level(s: &str) -> String {
    match s {
        "warning" => "warn".to_string(),
        other => other.to_string(),
    }
}

/// Stable, short content hash used as the `log_id` (locate a specific line).
///
/// TODO(sequence): PMP ingest exposes no stable per-line sequence number, so
/// `log_id` is a content hash (first 8 bytes of SHA256) rather than a monotonic
/// sequence. Log focus is therefore implemented as a filter, not a page jump; a
/// real sequence would allow exact page positioning across ring-buffer windows.
fn log_id_for(line: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(line.as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_array_and_object() {
        assert_eq!(parse_log_frames(&serde_json::json!(["a", "b"])), vec!["a", "b"]);
        assert_eq!(
            parse_log_frames(&serde_json::json!({"line": "x"})),
            vec!["x"]
        );
        assert!(parse_log_frames(&serde_json::json!(42)).is_empty());
    }

    #[test]
    fn history_lines_from_pmp_shape() {
        let v = serde_json::json!({ "lines": ["a", "b"], "count": 2 });
        assert_eq!(history_lines_of(&v), vec!["a", "b"]);
    }

    #[test]
    fn structures_unbracketed_line() {
        let e = parse_log_line("2026-08-13T10:00:00.123Z  INFO room::svc: created room ABC");
        assert_eq!(e.level, "info");
        assert_eq!(e.timestamp, "2026-08-13T10:00:00.123Z");
        assert!(e.message.contains("created room ABC"), "message: {}", e.message);
        assert_eq!(e.service, "pmp");
        assert_eq!(e.log_id.len(), 16);
    }

    #[test]
    fn structures_bracketed_line() {
        let e = parse_log_line("[2026-08-13T10:00:00Z] [ERROR] something failed");
        assert_eq!(e.level, "error");
        assert_eq!(e.timestamp, "2026-08-13T10:00:00Z");
        assert_eq!(e.message, "something failed");
    }

    #[test]
    fn keeps_plain_line_as_message() {
        let e = parse_log_line("just some text");
        assert_eq!(e.message, "just some text");
        assert_eq!(e.level, "info");
        assert_eq!(e.timestamp, "");
    }

    #[test]
    fn log_id_is_stable() {
        assert_eq!(log_id_for("same"), log_id_for("same"));
        assert_ne!(log_id_for("same"), log_id_for("other"));
    }
}
