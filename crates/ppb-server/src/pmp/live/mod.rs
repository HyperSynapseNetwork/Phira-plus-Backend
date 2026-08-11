//! Live gateway scaffold (design §11.4 / §12.6, Phase D).
//!
//! PMP high-frequency stream → PPB jitter buffer → PPF WS. Phase A defines the
//! configuration types and sequence-gap detection; the full gateway is Phase D.

use std::sync::Arc;

use dashmap::DashMap;

/// Jitter buffer mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JitterMode {
    #[default]
    LowLatency,
    Stable,
}

impl JitterMode {
    /// Default jitter buffer delay in milliseconds.
    pub fn default_delay_ms(&self) -> u64 {
        match self {
            Self::LowLatency => 1000,
            Self::Stable => 2000,
        }
    }
}

/// Per-room live stream state (sequence tracking).
#[derive(Debug, Default)]
struct RoomStreamState {
    last_sequence: Option<u64>,
}

/// Live gateway state (Phase A scaffold; Phase D wires WS output).
#[derive(Clone, Default)]
pub struct LiveGateway {
    mode: JitterMode,
    rooms: Arc<DashMap<String, RoomStreamState>>,
}

impl LiveGateway {
    pub fn new(mode: JitterMode) -> Self {
        Self {
            mode,
            rooms: Arc::new(DashMap::new()),
        }
    }

    pub fn mode(&self) -> JitterMode {
        self.mode
    }

    /// Record a frame's sequence for a room and report whether a gap was detected.
    /// Returns `Some(previous)` when a gap exists relative to the last sequence.
    pub fn observe_sequence(&self, room: &str, sequence: u64) -> Option<u64> {
        let mut entry = self.rooms.entry(room.to_string()).or_default();
        let prev = entry.last_sequence.replace(sequence);
        match prev {
            Some(p) if sequence > p + 1 => Some(p),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_defaults() {
        assert_eq!(JitterMode::LowLatency.default_delay_ms(), 1000);
        assert_eq!(JitterMode::Stable.default_delay_ms(), 2000);
    }

    #[test]
    fn detects_sequence_gap() {
        let gw = LiveGateway::new(JitterMode::LowLatency);
        assert!(gw.observe_sequence("r", 1).is_none());
        assert!(gw.observe_sequence("r", 2).is_none());
        // Gap: 2 -> 5.
        let gap = gw.observe_sequence("r", 5);
        assert_eq!(gap, Some(2));
        // Recovery.
        assert!(gw.observe_sequence("r", 6).is_none());
    }

    #[test]
    fn rooms_are_independent() {
        let gw = LiveGateway::new(JitterMode::Stable);
        gw.observe_sequence("a", 1);
        gw.observe_sequence("b", 1);
        let gap = gw.observe_sequence("a", 4);
        assert_eq!(gap, Some(1));
    }
}
