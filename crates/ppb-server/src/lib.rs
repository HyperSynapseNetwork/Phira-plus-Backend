//! PPB — Phira+ Backend (identity / community / control / integration plane).
//!
//! Domain-vertical modules (design §24.1). Each domain owns its model/service/
//! repo/routes; there is no global `models/` + `services/` dump.

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
