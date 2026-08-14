//! Logs domain.
//!
//! PMP provides `logs.history` / `logs.input` and the `logs` OpenUDS stream.
//! PPB keeps its own structured logs; the PMP Console path is via Action
//! Registry (`pmp.cli.execute`) and `/admin/logs/input`.

pub mod routes;
pub mod translator;

use serde_json::Value;

/// One PMP log occurrence as returned by `logs.history` / live `logs`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LogLine {
    #[serde(default)]
    pub seq: Option<u64>,
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

/// Convert a raw `logs` stream frame into sequenced occurrences. Legacy PMP
/// frames without `seq` remain readable and fall back to a content-derived id.
pub fn parse_log_occurrences(frames: &Value) -> Vec<LogLine> {
    fn one(value: &Value) -> Option<LogLine> {
        if let Some(line) = value.as_str() {
            return Some(LogLine { seq: None, line: line.to_string() });
        }
        let line = value.get("line")?.as_str()?.to_string();
        Some(LogLine { seq: value.get("seq").and_then(Value::as_u64), line })
    }
    match frames {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(frames).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Backward-compatible text-only helper.
pub fn parse_log_frames(frames: &Value) -> Vec<String> {
    parse_log_occurrences(frames).into_iter().map(|entry| entry.line).collect()
}

/// Extract occurrences from `logs.history`. New PMP returns
/// `{ entries:[{seq,line}], lines:[...], count }`; old PMP only returned lines.
pub fn history_entries_of(value: &Value) -> Vec<LogLine> {
    if let Some(entries) = value.get("entries").and_then(Value::as_array) {
        let parsed = entries.iter().filter_map(|value| {
            let line = value.get("line")?.as_str()?.to_string();
            Some(LogLine { seq: value.get("seq").and_then(Value::as_u64), line })
        }).collect::<Vec<_>>();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    match value.get("lines").and_then(Value::as_array) {
        Some(arr) => arr.iter().filter_map(|v| v.as_str().map(|line| LogLine { seq: None, line: line.to_string() })).collect(),
        None => parse_log_occurrences(value),
    }
}

/// Backward-compatible text-only history helper.
pub fn history_lines_of(value: &Value) -> Vec<String> {
    history_entries_of(value).into_iter().map(|entry| entry.line).collect()
}

/// Best-effort structure one raw PMP log occurrence. When PMP supplies `seq`,
/// `log_id` is `pmp-<seq>` and identifies this occurrence even if the text is
/// identical to another line. Legacy PMP falls back to a content hash.
pub fn parse_log_line_with_seq(line: &str, seq: Option<u64>) -> LogEntry {
    let trimmed = line.trim();
    let mut timestamp = String::new();
    let mut level = "info".to_string();
    let mut message = trimmed.to_string();
    let mut event = None;
    let mut error_code = None;
    let mut request_id = None;
    let mut command_id = None;
    let mut room_uuid = None;
    let mut user_id = None;

    // tracing JSON output: extract stable operational fields while preserving
    // the original text as fallback. Unknown fields remain intentionally opaque.
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        let fields = value.get("fields").unwrap_or(&value);
        timestamp = value.get("timestamp").and_then(Value::as_str).unwrap_or_default().to_string();
        level = value.get("level").and_then(Value::as_str).map(str::to_ascii_lowercase).map(|v| normalize_level(&v)).unwrap_or_else(|| "info".to_string());
        message = fields.get("message").or_else(|| value.get("message")).and_then(Value::as_str).unwrap_or(trimmed).to_string();
        event = string_field(fields, "event");
        error_code = string_field(fields, "error_code");
        request_id = string_field(fields, "request_id");
        command_id = string_field(fields, "command_id");
        room_uuid = string_field(fields, "room_uuid").or_else(|| string_field(fields, "room_id"));
        user_id = string_field(fields, "user_id");
    } else if let Some(rest) = trimmed.strip_prefix('[') {
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
        log_id: log_id_for(trimmed, seq),
        timestamp,
        service: "pmp".to_string(),
        level,
        event,
        message,
        error_code,
        request_id,
        command_id,
        room_uuid,
        user_id,
    }
}

pub fn parse_log_line(line: &str) -> LogEntry {
    parse_log_line_with_seq(line, None)
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
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

/// Stable occurrence id. New PMP sequence numbers are authoritative within one
/// process lifetime; legacy servers use a namespaced content hash as fallback.
fn log_id_for(line: &str, seq: Option<u64>) -> String {
    if let Some(seq) = seq {
        return format!("pmp-{seq}");
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(line.as_bytes());
    let suffix = digest[..8].iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("pmp-legacy-{suffix}")
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
        assert!(e.log_id.starts_with("pmp-legacy-"));
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
        assert_eq!(log_id_for("same", None), log_id_for("same", None));
        assert_ne!(log_id_for("same", None), log_id_for("other", None));
        assert_ne!(log_id_for("same", Some(1)), log_id_for("same", Some(2)));
    }
}
