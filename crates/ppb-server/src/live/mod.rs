//! Live Gateway (design §12.6, contract §4).
//!
//! PMP high-frequency stream → PPB jitter buffer → PPF WS. JSON envelope
//! mirroring the monitor-common `LiveEvent` semantics (touches/judges),
//! plus explicit `resync` / `round_switch` control messages.

pub mod routes;

pub use crate::pmp::live::JitterMode;
