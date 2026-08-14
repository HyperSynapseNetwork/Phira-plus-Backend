//! Metrics domain: in-memory operational counters exposed through a stable snapshot.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Lightweight counters for operational signals.
#[derive(Debug, Default)]
pub struct Metrics {
    pub commands_total: AtomicU64,
    pub commands_failed: AtomicU64,
    pub phira_api_errors: AtomicU64,
    pub openuds_reconnects: AtomicU64,
    pub events_forwarded: AtomicU64,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn incr(&self, counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "commands_total": self.commands_total.load(Ordering::Relaxed),
            "commands_failed": self.commands_failed.load(Ordering::Relaxed),
            "phira_api_errors": self.phira_api_errors.load(Ordering::Relaxed),
            "openuds_reconnects": self.openuds_reconnects.load(Ordering::Relaxed),
            "events_forwarded": self.events_forwarded.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_shape() {
        let m = Metrics::new();
        m.incr(&m.commands_total);
        let s = m.snapshot();
        assert_eq!(s["commands_total"], 1);
        assert_eq!(s["commands_failed"], 0);
    }
}
