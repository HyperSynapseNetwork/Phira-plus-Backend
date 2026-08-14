//! `/api/v1/admin/notifications/*` (design §18.13, contract §17).

use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::extractors::{ApiJson};
use crate::error::{ApiError, ErrorCode};
use crate::notifications::push::PushSummary;
use crate::notifications::NotificationActionDraft;

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
    pub actions: Vec<NotificationActionDraft>,
    #[serde(default)]
    pub dedup_key: String,
    #[serde(default)]
    pub payload: Value,
}

/// Typed `POST /admin/notifications/send` response `{event_id, recipients, push}`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NotificationSendResponse {
    pub event_id: Uuid,
    pub recipients: u64,
    pub push: PushSummary,
}

/// One delivery row consumed by Panel. `status` describes in-app fanout; push
/// delivery has a separate per-send summary and is not backfilled here.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NotificationDeliveryItem {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub notification_type: String,
    pub title: String,
    pub target_summary: String,
    pub status: String,
    pub delivered_count: i64,
    pub failed_count: i64,
    pub sent_at: chrono::DateTime<chrono::Utc>,
}

/// Typed `GET /admin/notifications/delivery` response `{items}`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct NotificationDeliveryResponse {
    pub items: Vec<NotificationDeliveryItem>,
}

/// POST /api/v1/admin/notifications/send — create an event + fan out inbox rows.
#[utoipa::path(
    post,
    path = "/api/v1/admin/notifications/send",
    operation_id = "admin_notifications_send_post",
    request_body = SendBody,
    responses(
        (status = 200, description = "event created + push summary", body = NotificationSendResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "notifications"
)]
pub async fn send(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<SendBody>,
) -> Result<Json<NotificationSendResponse>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "notification:send_system")
        .await?;
    let db = state.require_db()?;
    let actions = crate::notifications::normalize_action_drafts(body.actions)?;
    let mut payload = if body.payload.is_null() {
        json!({})
    } else if body.payload.is_object() {
        body.payload
    } else {
        return Err(ApiError::new(
            ErrorCode::ValidationFailed,
            "notification payload must be an object",
        ));
    };
    if let Some(object) = payload.as_object_mut() {
        object.insert("type".into(), json!(body.notification_type));
        object.insert("priority".into(), json!(body.priority));
        object.insert("title".into(), json!(body.title));
        object.insert("body".into(), json!(body.body));
        object.insert("actions".into(), json!(actions));
        object.insert("dedup_key".into(), json!(body.dedup_key));
        object.insert("admin_target".into(), body.target.clone());
    }
    let recipients = resolve_recipients(db, &body.target).await?;
    if recipients.is_empty() {
        return Err(ApiError::new(
            ErrorCode::ValidationFailed,
            "notification target resolves to no users",
        ));
    }
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
    let mut push_summary = PushSummary::default();
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
    Ok(Json(NotificationSendResponse {
        event_id: event.id,
        recipients: recipients.len() as u64,
        push: push_summary,
    }))
}

/// GET /api/v1/admin/notifications/delivery — recent events + delivered counts.
#[utoipa::path(
    get,
    path = "/api/v1/admin/notifications/delivery",
    operation_id = "admin_notifications_delivery_get",
    responses(
        (status = 200, description = "delivery summary", body = NotificationDeliveryResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn delivery(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<NotificationDeliveryResponse>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "notification:send_system")
        .await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, (Uuid, String, Value, chrono::DateTime<chrono::Utc>, i64)>(
        "SELECT ev.id, ev.type, ev.payload, ev.created_at, COUNT(un.id) AS delivered
         FROM notification_events ev
         LEFT JOIN user_notifications un ON un.event_id = ev.id
         GROUP BY ev.id
         ORDER BY ev.created_at DESC
         LIMIT 100",
    )
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let items = rows
        .into_iter()
        .map(|(id, typ, payload, created_at, delivered)| {
            let title = payload.get("title").and_then(Value::as_str).unwrap_or("").to_string();
            let target_summary = summarize_target(payload.get("admin_target"));
            NotificationDeliveryItem {
                id,
                notification_type: typ,
                title,
                target_summary,
                status: if delivered > 0 { "delivered".to_string() } else { "queued".to_string() },
                delivered_count: delivered,
                failed_count: 0,
                sent_at: created_at,
            }
        })
        .collect();
    Ok(Json(NotificationDeliveryResponse { items }))
}

fn summarize_target(target: Option<&Value>) -> String {
    let Some(target) = target else { return "—".to_string(); };
    if target.get("all").and_then(Value::as_bool).unwrap_or(false) {
        return "all".to_string();
    }
    let groups = target.get("group_ids").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    let users = target.get("user_ids").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
    if groups == 0 && users == 0 {
        "—".to_string()
    } else {
        format!("groups:{groups}; users:{users}")
    }
}

async fn resolve_recipients(db: &sqlx::PgPool, target: &Value) -> Result<Vec<Uuid>, ApiError> {
    if target.get("all").and_then(Value::as_bool).unwrap_or(false) {
        let rows = sqlx::query_as::<_, (Uuid,)>("SELECT id FROM users")
            .fetch_all(db)
            .await
            .map_err(db_err)?;
        return Ok(rows.into_iter().map(|r| r.0).collect());
    }
    if let Some(group_ids) = target.get("group_ids").and_then(Value::as_array) {
        let mut out = Vec::new();
        for value in group_ids {
            let Some(group_id) = value.as_str().and_then(|raw| Uuid::parse_str(raw).ok()) else {
                continue;
            };
            let rows = sqlx::query_as::<_, (Uuid,)>("SELECT user_id FROM group_members WHERE group_id = $1")
                .bind(group_id)
                .fetch_all(db)
                .await
                .map_err(db_err)?;
            out.extend(rows.into_iter().map(|row| row.0));
        }
        out.sort_unstable();
        out.dedup();
        return Ok(out);
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
        ApiError::new(ErrorCode::ResourceNotFound, "not found")
    } else {
        tracing::error!(error = %e, "notifications db error");
        ApiError::internal()
    }
}
