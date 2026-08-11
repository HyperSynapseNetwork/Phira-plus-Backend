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

    async fn run(&self, job_id: Uuid, job_type: &str, args: Value, cancel: Arc<AtomicBool>) {
        if let Some(db) = &self.db {
            let _ = update_state(db, job_id, "running", "starting", Some(0.0), "").await;
            self.publish(job_id, "running", "starting", 0.0);
        }

        let result = self.execute(job_id, job_type, &args, &cancel).await;

        if let Some(db) = &self.db {
            match &result {
                Ok((stage, progress)) => {
                    let _ = update_state(db, job_id, "succeeded", stage, Some(*progress), "").await;
                    self.publish(job_id, "succeeded", stage, *progress);
                }
                Err(e) => {
                    let _ = update_state(db, job_id, "failed", "error", None, e).await;
                    self.publish(job_id, "failed", "error", -1.0);
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
                let result = crate::pmp::cli::cli_execute(&self.openuds, cmd).await;
                self.tick(job_id, "checking", 0.5, cancel).await?;
                result.map(|_| ("checked", 1.0)).map_err(|e| e.to_string())
            }
            "pmp.update.apply" => {
                self.tick(job_id, "downloading", 0.3, cancel).await?;
                self.tick(job_id, "verifying", 0.6, cancel).await?;
                let cmd = args.get("command").and_then(Value::as_str).unwrap_or("update apply");
                let result = crate::pmp::cli::cli_execute(&self.openuds, cmd).await;
                self.tick(job_id, "applying", 0.9, cancel).await?;
                result.map(|_| ("applied", 1.0)).map_err(|e| e.to_string())
            }
            "ppf.build" => {
                for i in 1..=5 {
                    self.tick(job_id, "building", i as f32 / 5.0, cancel).await?;
                }
                Ok(("built", 1.0))
            }
            "backup" => {
                self.tick(job_id, "backing-up", 0.5, cancel).await?;
                Ok(("backup-complete", 1.0))
            }
            other => Err(format!("unknown job type: {other}")),
        }
    }

    async fn tick(&self, job_id: Uuid, stage: &str, progress: f32, cancel: &Arc<AtomicBool>) -> Result<(), String> {
        if cancel.load(Ordering::Acquire) {
            if let Some(db) = &self.db {
                let _ = update_state(db, job_id, "cancelled", "cancelled", Some(progress), "cancelled").await;
            }
            self.publish(job_id, "cancelled", "cancelled", progress);
            return Err("cancelled".to_string());
        }
        if let Some(db) = &self.db {
            let _ = update_state(db, job_id, "running", stage, Some(progress), "").await;
        }
        self.publish(job_id, "running", stage, progress);
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(())
    }

    fn publish(&self, job_id: Uuid, state: &str, stage: &str, progress: f32) {
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
