//! Admin namespace routes.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::{self, Stream};
use futures_util::StreamExt;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::actions::routes as action_routes;
use crate::app::AppState;
use crate::auth::routes as auth_routes;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;
use crate::permissions::routes as permission_routes;
use crate::pmp::events::{PpbEvent, ReplayResult};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(admin_events_sse))
        .route("/server/status", get(server_status))
        .route("/audit", get(audit_list))
        .route("/jobs", get(jobs_list))
        .merge(auth_routes::root_routes())
        .merge(permission_routes::routes())
        .merge(action_routes::routes())
}

/// GET /api/v1/admin/events — admin control-plane SSE.
pub async fn admin_events_sse(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "dashboard:view")
        .await?;

    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let rx = state.events.subscribe();

    let replay = match last_event_id {
        Some(id) => match state.events.replay_from(&id) {
            ReplayResult::Events(events) => events,
            ReplayResult::Miss => state.events.snapshot(),
        },
        None => Vec::new(),
    };

    let replay_stream = stream::iter(replay.into_iter().map(Ok::<_, Infallible>));
    let live_stream = BroadcastStream::new(rx)
        .filter_map(|r| r.ok())
        .map(Ok::<_, Infallible>);

    let stream = replay_stream.chain(live_stream).map(|item| {
        item.map(|ev: Arc<PpbEvent>| {
            Event::default()
                .id(ev.id.clone())
                .event(ev.event_type.clone())
                .data(serde_json::to_string(&*ev).unwrap_or_else(|_| "{}".to_string()))
        })
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// GET /api/v1/admin/server/status — scaffold summary.
pub async fn server_status(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "server:view")
        .await?;
    let openuds_state = state.openuds.state().await;
    Ok(Json(json!({
        "ppb_version": env!("CARGO_PKG_VERSION"),
        "pmp": {
            "connected": openuds_state.connected,
            "version": openuds_state.server_version,
            "session_id": openuds_state.session_id,
        },
        "db_configured": state.db.is_some(),
        "metrics": state.metrics.snapshot(),
    })))
}

/// GET /api/v1/admin/audit — recent audit events.
pub async fn audit_list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::audit::model::AuditEvent>>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "audit:view")
        .await?;
    let db = state.require_db()?;
    let events = crate::audit::repo::list(db, 200).await?;
    Ok(Json(events))
}

/// GET /api/v1/admin/jobs — recent jobs.
pub async fn jobs_list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::jobs::Job>>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "dashboard:view")
        .await?;
    let db = state.require_db()?;
    let jobs = sqlx::query_as::<_, crate::jobs::Job>(
        "SELECT id, type, state, progress, stage, created_at, started_at, finished_at, error
         FROM jobs ORDER BY created_at DESC LIMIT 100",
    )
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "jobs query failed");
        ApiError::internal()
    })?;
    Ok(Json(jobs))
}
