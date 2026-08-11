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
}
