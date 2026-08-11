//! PPB — Phira+ Backend (identity / community / control / integration plane).
//!
//! Domain-vertical modules (design §24.1). Each domain owns its model/service/
//! repo/routes; there is no global `models/` + `services/` dump.

// Clippy stylistic allows (warn-by-default lints that don't indicate bugs).
// - uninlined_format_args: the codebase mixes positional and inlined format
//   args; this is stylistic and not worth churn on a foundation.
// - new_without_default: many services require config arguments and
//   deliberately do not implement Default.
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::new_without_default)]

pub mod actions;
pub mod admin;
pub mod app;
pub mod audit;
pub mod auth;
pub mod automation;
pub mod commands;
pub mod config;
pub mod error;
pub mod identities;
pub mod jobs;
pub mod logs;
pub mod metrics;
pub mod middleware;
pub mod notifications;
pub mod permissions;
pub mod phira;
pub mod pmp;
pub mod preferences;
pub mod public;
pub mod replay;
pub mod rooms;
pub mod social;
pub mod telemetry;
pub mod users;
