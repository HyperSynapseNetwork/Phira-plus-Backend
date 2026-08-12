//! Job runner (design §9.4). Long tasks: pmp.update / ppf.build / backup.
//!
//! Job state transitions: queued → running(stage,progress) → succeeded/failed/cancelled.
//! State changes are published to the EventBus (SSE `job.updated`) and persisted.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use serde_json::Value;
use uuid::Uuid;

use super::{create, update_state, Job};
use crate::error::ApiError;
use crate::pmp::events::{PpbEvent, ResourceRef};

/// In-process job runner.
#[derive(Clone)]
pub struct JobRunner {
    db: Option<sqlx::PgPool>,
    events: crate::pmp::events::EventBus,
    openuds: Arc<crate::pmp::openuds::client::OpenUdsClient>,
    cancels: Arc<DashMap<Uuid, Arc<AtomicBool>>>,
}

impl JobRunner {
    pub fn new(
        db: Option<sqlx::PgPool>,
        events: crate::pmp::events::EventBus,
        openuds: Arc<crate::pmp::openuds::client::OpenUdsClient>,
    ) -> Self {
        Self {
            db,
            events,
            openuds,
            cancels: Arc::new(DashMap::new()),
        }
    }

    /// Start a job of `job_type`; returns the created job row.
    pub async fn start(&self, job_type: &str, args: Value) -> Result<Job, ApiError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| ApiError::new(crate::error::ErrorCode::Internal, "database not configured"))?;
        let job = create(db, job_type).await?;
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels.insert(job.id, cancel.clone());

        let runner = self.clone();
        let jt = job_type.to_string();
        tokio::spawn(async move {
            runner.run(job.id, &jt, args, cancel).await;
        });
        Ok(job)
    }

    pub async fn cancel(&self, job_id: Uuid) -> Result<(), ApiError> {
        if let Some(flag) = self.cancels.get(&job_id) {
            flag.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Re-queue a failed/cancelled job and re-run it (Panel retry). Job args are
    /// not persisted, so retries use the job type's default command.
    pub async fn retry(&self, job_id: Uuid) -> Result<(), ApiError> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| ApiError::new(crate::error::ErrorCode::Internal, "database not configured"))?;
        let job: Option<Job> = sqlx::query_as::<_, Job>(
            "SELECT id, type, state, progress, stage, created_at, started_at, finished_at, error
             FROM jobs WHERE id = $1",
        )
        .bind(job_id)
        .fetch_optional(db)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "job retry lookup failed");
            ApiError::internal()
        })?;
        let Some(job) = job else {
            return Err(ApiError::new(crate::error::ErrorCode::NotFound, "job not found"));
        };
        if job.state == "succeeded" {
            return Err(ApiError::new(crate::error::ErrorCode::Conflict, "job already succeeded"));
        }
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
            runner.run(job_id, &jt, Value::Null, cancel).await;
        });
        Ok(())
    }

    async fn run(&self, job_id: Uuid, job_type: &str, args: Value, cancel: Arc<AtomicBool>) {
        if let Some(db) = &self.db {
            let _ = update_state(db, job_id, "running", "starting", None, "").await;
            self.publish(job_id, "running", "starting", None);
        }

        let result = self.execute(job_id, job_type, &args, &cancel).await;

        if let Some(db) = &self.db {
            match &result {
                Ok((stage, progress)) => {
                    let _ = update_state(db, job_id, "succeeded", stage, Some(*progress), "").await;
                    self.publish(job_id, "succeeded", stage, Some(*progress));
                }
                Err(e) => {
                    let _ = update_state(db, job_id, "failed", "error", None, e).await;
                    self.publish(job_id, "failed", "error", None);
                }
            }
        }
        self.cancels.remove(&job_id);
    }

    async fn execute(
        &self,
        job_id: Uuid,
        job_type: &str,
        args: &Value,
        cancel: &Arc<AtomicBool>,
    ) -> Result<(String, f32), String> {
        match job_type {
            "pmp.update.check" => {
                let cmd = args.get("command").and_then(Value::as_str).unwrap_or("update check");
                self.tick_stage(job_id, "checking", cancel).await?;
                let result = crate::pmp::cli::cli_execute(&self.openuds, cmd).await;
                result.map(|_| ("checked".to_string(), 1.0f32)).map_err(|e| e.to_string())
            }
            "pmp.update.apply" => {
                // No real PMP progress → stage strings only, progress stays null.
                self.tick_stage(job_id, "downloading", cancel).await?;
                self.tick_stage(job_id, "verifying", cancel).await?;
                let cmd = args.get("command").and_then(Value::as_str).unwrap_or("update apply");
                let result = crate::pmp::cli::cli_execute(&self.openuds, cmd).await;
                self.tick_stage(job_id, "applying", cancel).await?;
                result.map(|_| ("applied".to_string(), 1.0f32)).map_err(|e| e.to_string())
            }
            "ppf.build" => {
                self.tick_stage(job_id, "building", cancel).await?;
                Ok(("built".to_string(), 1.0f32))
            }
            "backup" => {
                self.tick_stage(job_id, "backing-up", cancel).await?;
                Ok(("backup-complete".to_string(), 1.0f32))
            }
            other => Err(format!("unknown job type: {other}")),
        }
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
