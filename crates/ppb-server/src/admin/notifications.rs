//! `/api/v1/admin/notifications/*` (design §18.13, contract §17).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/notifications/send", post(send))
        .route("/notifications/delivery", get(delivery))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SendBody {
    #[serde(rename = "type")]
    pub notification_type: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub priority: String,
    /// `{ "all": true }` | `{ "group_id": "<uuid>" }` | `{ "user_ids": [...] }`
    #[serde(default)]
    pub target: Value,
    #[serde(default)]
    pub payload: Value,
}

/// POST /api/v1/admin/notifications/send — create an event + fan out inbox rows.
#[utoipa::path(
    post,
    path = "/api/v1/admin/notifications/send",
    request_body = SendBody,
    responses(
        (status = 200, description = "event created + push summary", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn send(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<SendBody>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "notification:send_system")
        .await?;
    let db = state.require_db()?;
    let payload = if body.payload.is_null() {
        json!({
            "type": body.notification_type,
            "priority": body.priority,
            "title": body.title,
            "body": body.body,
        })
    } else {
        body.payload
    };
    let recipients = resolve_recipients(db, &body.target).await?;
    let event = crate::notifications::publish_to_users(
        db,
        &body.notification_type,
        Some(auth.sub),
        payload.clone(),
        &recipients,
    )
    .await?;
    // Fan out push (in-app rows already created above); per-endpoint failures
    // are non-fatal and summarized.
    let mut push_summary = crate::notifications::push::PushSummary::default();
    for uid in &recipients {
        let r = state
            .push
            .notify(db, *uid, &body.title, &body.body, Some(&payload))
            .await;
        if let Ok(s) = r {
            push_summary.delivered += s.delivered;
            push_summary.not_configured += s.not_configured;
            push_summary.failed += s.failed;
        }
    }
    Ok(Json(json!({
        "event_id": event.id,
        "recipients": recipients.len(),
        "push": push_summary,
    })))
}

/// GET /api/v1/admin/notifications/delivery — recent events + delivered counts.
#[utoipa::path(
    get,
    path = "/api/v1/admin/notifications/delivery",
    responses(
        (status = 200, description = "delivery summary", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn delivery(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "notification:send_system")
        .await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, (Uuid, String, chrono::DateTime<chrono::Utc>, i64)>(
        "SELECT ev.id, ev.type, ev.created_at, COUNT(un.id) AS delivered
         FROM notification_events ev
         LEFT JOIN user_notifications un ON un.event_id = ev.id
         GROUP BY ev.id
         ORDER BY ev.created_at DESC
         LIMIT 100",
    )
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, typ, created_at, delivered)| {
            json!({ "event_id": id, "type": typ, "created_at": created_at, "delivered": delivered })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

async fn resolve_recipients(db: &sqlx::PgPool, target: &Value) -> Result<Vec<Uuid>, ApiError> {
    if target.get("all").and_then(Value::as_bool).unwrap_or(false) {
        let rows = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM users")
            .fetch_all(db)
            .await
            .map_err(db_err)?;
        return Ok(rows.into_iter().map(|r| r.0).collect());
    }
    if let Some(gid) = target.get("group_id").and_then(Value::as_str) {
        if let Ok(group_id) = Uuid::parse_str(gid) {
            let rows = sqlx::query_as::<_, (Uuid,)>("SELECT user_id FROM group_members WHERE group_id = $1")
                .bind(group_id)
                .fetch_all(db)
                .await
                .map_err(db_err)?;
            return Ok(rows.into_iter().map(|r| r.0).collect());
        }
    }
    if let Some(ids) = target.get("user_ids").and_then(Value::as_array) {
        let mut out = Vec::new();
        for v in ids {
            if let Some(u) = v.as_str().and_then(|s| Uuid::parse_str(s).ok()) {
                out.push(u);
            }
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "not found")
    } else {
        tracing::error!(error = %e, "notifications db error");
        ApiError::internal()
    }
}
