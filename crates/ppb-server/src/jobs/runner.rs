//! Job runner (design §9.4). Long tasks: pmp.update / ppf.build / backup.
//!
//! Job state transitions: queued → running(stage,progress) →
//! succeeded/failed/cancelled/not_implemented.
//! State changes are published to the EventBus (SSE `job.updated`) and persisted.
//!
//! Executors come from the Job Policy Registry (`super::registry`) — a
//! server-fixed `FixedCli` command, never client-supplied CLI text.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use uuid::Uuid;

use super::registry::{CancelMode, JobDescriptor, JobExecutor, JobRegistry};
use super::{create, update_state, Job};
use crate::error::{ApiError, ErrorCode};
use crate::pmp::events::{PpbEvent, ResourceRef};

/// In-process job runner.
#[derive(Clone)]
pub struct JobRunner {
    db: Option<sqlx::PgPool>,
    events: crate::pmp::events::EventBus,
    openuds: Arc<crate::pmp::openuds::client::OpenUdsClient>,
    registry: Arc<JobRegistry>,
    cancels: Arc<DashMap<Uuid, Arc<AtomicBool>>>,
}

impl JobRunner {
    pub fn new(
        db: Option<sqlx::PgPool>,
        events: crate::pmp::events::EventBus,
        openuds: Arc<crate::pmp::openuds::client::OpenUdsClient>,
        registry: Arc<JobRegistry>,
    ) -> Self {
        Self {
            db,
            events,
            openuds,
            registry,
            cancels: Arc::new(DashMap::new()),
        }
    }

    /// Look up a job's descriptor (for Create/Retry/Cancel permission gates).
    pub async fn descriptor(&self, job_id: Uuid) -> Result<&'static JobDescriptor, ApiError> {
        let db = self.require_db()?;
        let job = self.lookup(db, job_id).await?;
        self.registry
            .get(&job.r#type)
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "job type missing from registry"))
    }

    /// Start a job of `job_type`; returns the created job row.
    pub async fn start(&self, job_type: &str) -> Result<Job, ApiError> {
        let descriptor = self
            .registry
            .get(job_type)
            .ok_or_else(|| ApiError::validation("unknown job type"))?;
        let db = self.require_db()?;
        self.ensure_resource_free(db, descriptor.resource_key).await?;

        let resource_key = descriptor.resource_key.unwrap_or("");
        let job = create(db, job_type, resource_key).await?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels.insert(job.id, cancel.clone());

        let runner = self.clone();
        let jt = job_type.to_string();
        tokio::spawn(async move {
            runner.run(job.id, &jt, cancel).await;
        });
        Ok(job)
    }

    /// Cancel a queued job. `cancel_mode: before_dispatch` means a job that has
    /// already been dispatched to PMP can no longer be cancelled — the runner
    /// never re-checks the flag after `cli_execute` starts, so claiming success
    /// here would be a lie.
    pub async fn cancel(&self, job_id: Uuid) -> Result<(), ApiError> {
        let db = self.require_db()?;
        let job = self.lookup(db, job_id).await?;
        let descriptor = self
            .registry
            .get(&job.r#type)
            .ok_or_else(|| ApiError::validation("unknown job type"))?;

        match descriptor.cancel_mode {
            CancelMode::BeforeDispatch => match job.state.as_str() {
                "queued" => {
                    if let Some(flag) = self.cancels.get(&job_id) {
                        flag.store(true, Ordering::Release);
                    }
                    Ok(())
                }
                "running" => Err(ApiError::new(
                    ErrorCode::Conflict,
                    "job already dispatched; cannot cancel",
                )),
                "succeeded" | "failed" | "cancelled" | "not_implemented" => Err(ApiError::new(
                    ErrorCode::Conflict,
                    "job already finished",
                )),
                _ => Err(ApiError::new(ErrorCode::Conflict, "job cannot be cancelled")),
            },
        }
    }

    /// Re-queue a failed/cancelled job and re-run it (Panel retry). Job args are
    /// not persisted; retries re-run the registry's fixed command.
    pub async fn retry(&self, job_id: Uuid) -> Result<(), ApiError> {
        let db = self.require_db()?;
        let job = self.lookup(db, job_id).await?;
        if job.state != "failed" && job.state != "cancelled" {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "only failed or cancelled jobs can be retried",
            ));
        }
        let descriptor = self
            .registry
            .get(&job.r#type)
            .ok_or_else(|| ApiError::validation("unknown job type"))?;
        self.ensure_resource_free(db, descriptor.resource_key).await?;

        sqlx::query(
            "UPDATE jobs SET state = 'queued', stage = '', progress = NULL, error = '',
             started_at = NULL, finished_at = NULL WHERE id = $1",
        )
        .bind(job_id)
        .execute(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "job retry reset failed");
            ApiError::internal()
        })?;

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels.insert(job_id, cancel.clone());
        let runner = self.clone();
        let jt = job.r#type.clone();
        tokio::spawn(async move {
            runner.run(job_id, &jt, cancel).await;
        });
        Ok(())
    }

    async fn run(&self, job_id: Uuid, job_type: &str, cancel: Arc<AtomicBool>) {
        if let Some(db) = &self.db {
            let _ = update_state(db, job_id, "running", "starting", None, "").await;
            self.publish(job_id, "running", "starting", None);
        }

        let result = self.execute(job_id, job_type, &cancel).await;

        if let Some(db) = &self.db {
            match &result {
                Ok((stage, _progress)) => {
                    let _ = update_state(db, job_id, "succeeded", stage, None, "").await;
                    self.publish(job_id, "succeeded", stage, None);
                }
                Err(e) if e == "not_implemented" => {
                    let _ = update_state(db, job_id, "not_implemented", "not_implemented", None, e).await;
                    self.publish(job_id, "not_implemented", "not_implemented", None);
                }
                Err(e) => {
                    // §23 #7: terminal stage is `failed` or `timeout`, not a
                    // hardcoded `error`.
                    let stage = if e == "timeout" { "timeout" } else { "failed" };
                    let _ = update_state(db, job_id, "failed", stage, None, e).await;
                    self.publish(job_id, "failed", stage, None);
                }
            }
        }
        self.cancels.remove(&job_id);
    }

    async fn execute(
        &self,
        job_id: Uuid,
        job_type: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(String, Option<f32>), String> {
        let descriptor = self
            .registry
            .get(job_type)
            .ok_or_else(|| format!("unknown job type: {job_type}"))?;
        let cmd = match descriptor.executor {
            JobExecutor::FixedCli(cmd) => cmd,
            JobExecutor::NotImplemented => return Err("not_implemented".to_string()),
        };

        // Cancel is only honoured before dispatch (§23): the flag is checked
        // here, then never again once `cli_execute` starts.
        self.tick_stage(job_id, descriptor.stage, cancel).await?;

        // No short overall timeout: `cli.execute` is already in flight with PMP.
        // A PPB-side timeout would only stop waiting while PMP keeps running,
        // making "timeout → retry" overlap two update chains. The connection
        // dropping is the only explicit failure; the real download timeout is
        // PMP's own concern. progress stays null until PMP actually returns.
        crate::pmp::cli::cli_execute(&self.openuds, cmd)
            .await
            .map(|_| (descriptor.terminal.to_string(), None))
            .map_err(|e| format!("failed: {e}"))
    }

    /// Advance a job's stage without faking a numeric progress value (§22:
    /// `progress=null` when PMP provides no real progress).
    async fn tick_stage(&self, job_id: Uuid, stage: &str, cancel: &Arc<AtomicBool>) -> Result<(), String> {
        if cancel.load(Ordering::Acquire) {
            if let Some(db) = &self.db {
                let _ = update_state(db, job_id, "cancelled", "cancelled", None, "cancelled").await;
            }
            self.publish(job_id, "cancelled", "cancelled", None);
            return Err("cancelled".to_string());
        }
        if let Some(db) = &self.db {
            let _ = update_state(db, job_id, "running", stage, None, "").await;
        }
        self.publish(job_id, "running", stage, None);
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }

    async fn ensure_resource_free(
        &self,
        db: &sqlx::PgPool,
        resource_key: Option<&str>,
    ) -> Result<(), ApiError> {
        let Some(key) = resource_key else {
            return Ok(());
        };
        let (active,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM jobs WHERE resource_key = $1 AND state IN ('queued','running')")
                .bind(key)
                .fetch_one(db)
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "job resource check failed");
                    ApiError::internal()
                })?;
        if active > 0 {
            return Err(ApiError::new(
                ErrorCode::Conflict,
                "job already running for this resource",
            ));
        }
        Ok(())
    }

    fn require_db(&self) -> Result<&sqlx::PgPool, ApiError> {
        self.db
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "database not configured"))
    }

    async fn lookup(&self, db: &sqlx::PgPool, job_id: Uuid) -> Result<Job, ApiError> {
        sqlx::query_as::<_, Job>(
            "SELECT id, type, state, progress, stage, created_at, started_at, finished_at, error
             FROM jobs WHERE id = $1",
        )
        .bind(job_id)
        .fetch_optional(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "job lookup failed");
            ApiError::internal()
        })?
        .ok_or_else(|| ApiError::not_found("job"))
    }

    fn publish(&self, job_id: Uuid, state: &str, stage: &str, progress: Option<f32>) {
        self.events.publish(PpbEvent {
            id: Uuid::new_v4().to_string(),
            event_type: "job.updated".to_string(),
            version: 1,
            occurred_at: chrono::Utc::now(),
            resource: ResourceRef {
                resource_type: "job".to_string(),
                id: job_id.to_string(),
            },
            data: serde_json::json!({ "state": state, "stage": stage, "progress": progress }),
        });
    }
}
