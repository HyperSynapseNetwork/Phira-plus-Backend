//! Phira Aggregator base worker (design §15.8 / TopChart).
//!
//! Periodically snapshots record counts of popular charts into `chart_snapshots`
//! (hourly buckets). Incremental deltas are derived on-demand. This is a base
//! worker; resource-isolated from the realtime PMP command path.

use std::sync::Arc;

use serde_json::Value;

use super::gateway::PhiraGateway;
use crate::error::ApiError;

pub struct Aggregator {
    db: Option<sqlx::PgPool>,
    gateway: Arc<PhiraGateway>,
    top_n: i32,
}

impl Aggregator {
    pub fn new(db: Option<sqlx::PgPool>, gateway: Arc<PhiraGateway>, top_n: i32) -> Self {
        Self { db, gateway, top_n }
    }

    /// Spawn the periodic worker.
    pub fn spawn(self: Arc<Self>, interval_hours: u64) {
        tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(interval_hours.max(1) * 3600);
            loop {
                self.run().await;
                tokio::time::sleep(interval).await;
            }
        });
    }

    async fn run(&self) {
        // Fetch popular charts (top N).
        let popular = match self.gateway.chart_popular(self.top_n.max(1) as i64).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "aggregator: popular fetch failed");
                return;
            }
        };
        let ids = extract_chart_ids(&popular);
        tracing::info!(count = ids.len(), "aggregator: snapshotting popular charts");

        let Some(db) = &self.db else { return };
        for id in ids {
            match self.gateway.record_query(id, 1, 1).await {
                Ok(r) => {
                    let count = r.get("count").and_then(Value::as_i64).unwrap_or(0);
                    if let Err(e) = record_snapshot(db, id, count).await {
                        tracing::warn!(chart = id, error = %e, "aggregator: snapshot failed");
                    }
                }
                Err(e) => tracing::debug!(chart = id, error = %e, "aggregator: record query skipped"),
            }
        }
    }
}

fn extract_chart_ids(popular: &Value) -> Vec<i64> {
    popular
        .get("results")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| c.get("id").and_then(Value::as_i64))
                .collect()
        })
        .unwrap_or_default()
}

async fn record_snapshot(db: &sqlx::PgPool, chart_id: i64, record_count: i64) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO chart_snapshots (chart_id, record_count, snapshot_at)
         VALUES ($1, $2, date_trunc('hour', now()))
         ON CONFLICT (chart_id, snapshot_at) DO UPDATE SET record_count = EXCLUDED.record_count",
    )
    .bind(chart_id)
    .bind(record_count)
    .execute(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "chart snapshot insert failed");
        ApiError::internal()
    })?;
    Ok(())
}
