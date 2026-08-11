//! Action Registry descriptor types (design §9.1, contract §6).

use serde::Serialize;

/// Executor backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Executor {
    /// Typed OpenUDS command.
    OpenUds,
    /// Wrapped `cli.execute` for an operation without a typed command.
    CliExecute,
    /// Raw PMP console (`cli.execute` with arbitrary input, full audit).
    CliRaw,
    /// Internal PPB operation.
    Internal,
}

/// Risk level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

/// One action descriptor.
#[derive(Debug, Clone, Serialize)]
pub struct ActionDescriptor {
    pub id: &'static str,
    pub permission: &'static str,
    pub executor: Executor,
    pub risk: Risk,
    pub audit: bool,
    pub reauth: bool,
    #[serde(rename = "host_allowed")]
    pub host_allowed: bool,
    #[serde(rename = "queue_key")]
    pub queue_key: &'static str,
    #[serde(rename = "long_running")]
    pub long_running: bool,
}

impl ActionDescriptor {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        id: &'static str,
        permission: &'static str,
        executor: Executor,
        risk: Risk,
        audit: bool,
        reauth: bool,
        host_allowed: bool,
        queue_key: &'static str,
        long_running: bool,
    ) -> Self {
        Self {
            id,
            permission,
            executor,
            risk,
            audit,
            reauth,
            host_allowed,
            queue_key,
            long_running,
        }
    }
}
