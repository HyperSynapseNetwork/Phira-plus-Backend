//! Job Policy Registry — single source of truth for Job permission / reauth /
//! resource_key / cancel_mode / executor (design §9.4).
//!
//! Stop-ship: this closes the "second execution plane" that let `POST /jobs`
//! run arbitrary PMP CLI text (`args.command`) while only checking
//! `server:update` (no reauth, no Action Registry). Every Job now has a
//! server-fixed executor (`FixedCli`); clients can never supply CLI text.

use std::collections::HashMap;

use crate::auth::reauth::ReauthRisk;

/// How a Job is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobExecutor {
    /// Server-side fixed CLI command. Client text is never accepted.
    FixedCli(&'static str),
    /// Not yet implemented; must terminate `not_implemented` rather than fake
    /// `succeeded`.
    NotImplemented,
}

/// When a Job may be cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelMode {
    /// Only while still queued (before the CLI command is dispatched).
    /// Once dispatched the runner never re-checks the cancel flag, so a
    /// "requested cancel" after dispatch would be a lie.
    BeforeDispatch,
}

/// One Job descriptor: everything Create / Retry / Cancel read from.
#[derive(Debug, Clone, Copy)]
pub struct JobDescriptor {
    pub id: &'static str,
    pub permission: &'static str,
    /// Reauth risk required to create/retry; `None` = no reauth.
    pub reauth: Option<ReauthRisk>,
    /// Parallelism-exclusion key; jobs sharing a non-empty key are mutually
    /// exclusive (one active at a time).
    pub resource_key: Option<&'static str>,
    pub retryable: bool,
    pub cancel_mode: CancelMode,
    pub executor: JobExecutor,
    /// Running stage published before dispatch (progress stays null).
    pub stage: &'static str,
    /// Terminal stage reported on success.
    pub terminal: &'static str,
}

/// Seed Jobs (design §9.4). `pmp.update.*` share `resource_key = "server"` so
/// an update check/apply/cancel/force can never overlap.
pub fn seed_jobs() -> Vec<JobDescriptor> {
    use JobExecutor::*;
    vec![
        JobDescriptor {
            id: "pmp.update.check",
            permission: "server:update",
            reauth: None,
            resource_key: Some("server"),
            retryable: true,
            cancel_mode: CancelMode::BeforeDispatch,
            executor: FixedCli("update check"),
            stage: "checking",
            terminal: "checked",
        },
        JobDescriptor {
            id: "pmp.update.apply",
            permission: "server:update",
            reauth: Some(ReauthRisk::Critical),
            resource_key: Some("server"),
            retryable: true,
            cancel_mode: CancelMode::BeforeDispatch,
            executor: FixedCli("update apply"),
            stage: "executing_pmp_update",
            terminal: "completed",
        },
        JobDescriptor {
            id: "pmp.update.cancel",
            permission: "server:update",
            reauth: None,
            resource_key: Some("server"),
            retryable: false,
            cancel_mode: CancelMode::BeforeDispatch,
            executor: FixedCli("update cancel"),
            stage: "cancelling_update",
            terminal: "completed",
        },
        JobDescriptor {
            id: "pmp.update.force",
            permission: "server:update",
            reauth: Some(ReauthRisk::Critical),
            resource_key: Some("server"),
            retryable: false,
            cancel_mode: CancelMode::BeforeDispatch,
            executor: FixedCli("update force"),
            stage: "forcing_update",
            terminal: "completed",
        },
        // Stubs — never fabricate success; terminate `not_implemented`.
        JobDescriptor {
            id: "ppf.build",
            permission: "server:manage",
            reauth: None,
            resource_key: Some("ppf"),
            retryable: false,
            cancel_mode: CancelMode::BeforeDispatch,
            executor: NotImplemented,
            stage: "building",
            terminal: "not_implemented",
        },
        JobDescriptor {
            id: "backup",
            permission: "server:manage",
            reauth: None,
            resource_key: Some("server"),
            retryable: false,
            cancel_mode: CancelMode::BeforeDispatch,
            executor: NotImplemented,
            stage: "backing-up",
            terminal: "not_implemented",
        },
    ]
}

/// Registry of Jobs keyed by id.
#[derive(Debug, Clone, Default)]
pub struct JobRegistry {
    jobs: HashMap<&'static str, &'static JobDescriptor>,
}

impl JobRegistry {
    pub fn new() -> Self {
        let mut registry = Self::default();
        for job in seed_jobs() {
            let leaked: &'static JobDescriptor = Box::leak(Box::new(job));
            registry.jobs.insert(leaked.id, leaked);
        }
        registry
    }

    pub fn get(&self, id: &str) -> Option<&'static JobDescriptor> {
        self.jobs.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_present_and_fixed() {
        let reg = JobRegistry::new();
        let apply = reg.get("pmp.update.apply").unwrap();
        assert_eq!(apply.permission, "server:update");
        assert_eq!(apply.reauth, Some(ReauthRisk::Critical));
        assert_eq!(apply.resource_key, Some("server"));
        assert_eq!(apply.executor, JobExecutor::FixedCli("update apply"));
        assert_eq!(apply.cancel_mode, CancelMode::BeforeDispatch);
        assert!(apply.retryable);

        assert_eq!(reg.get("pmp.update.check").unwrap().executor, JobExecutor::FixedCli("update check"));
        assert_eq!(reg.get("pmp.update.cancel").unwrap().executor, JobExecutor::FixedCli("update cancel"));
        assert_eq!(reg.get("pmp.update.force").unwrap().executor, JobExecutor::FixedCli("update force"));
    }

    #[test]
    fn update_jobs_share_server_resource_key() {
        let reg = JobRegistry::new();
        for id in ["pmp.update.check", "pmp.update.apply", "pmp.update.cancel", "pmp.update.force"] {
            assert_eq!(reg.get(id).unwrap().resource_key, Some("server"), "{id} must share server key");
        }
    }

    #[test]
    fn stubs_are_not_implemented() {
        let reg = JobRegistry::new();
        assert_eq!(reg.get("ppf.build").unwrap().executor, JobExecutor::NotImplemented);
        assert_eq!(reg.get("backup").unwrap().executor, JobExecutor::NotImplemented);
    }
}
