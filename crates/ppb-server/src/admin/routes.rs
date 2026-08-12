//! Admin namespace routes.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::{self, Stream};
use futures_util::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::actions::routes as action_routes;
use crate::app::AppState;
use crate::auth::routes as auth_routes;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorEnvelope};
use crate::permissions::routes as permission_routes;
use crate::pmp::events::{PpbEvent, ReplayResult};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/events", get(admin_events_sse))
        .route("/server/status", get(server_status))
        // Alias: Panel `fetchServerStatus` calls `/admin/server`.
        .route("/server", get(server_status))
        // Alias: Panel `fetchPermissionManifest` calls `/admin/permissions`.
        .route("/permissions", get(crate::permissions::routes::manifest))
        // Alias: Panel `fetchRunbookRuns` calls `/admin/runbook-runs`.
        .route("/runbook-runs", get(crate::automation::routes::runs))
        .route("/auth/reauth", post(crate::auth::routes::phira_reauth))
        .merge(crate::audit::routes::routes())
        .merge(crate::config::routes::routes())
        .merge(crate::logs::routes::routes())
        .merge(super::server::routes())
        .merge(super::plugins::routes())
        .merge(super::notifications::routes())
        .merge(super::coupons::routes())
        .merge(crate::automation::routes::routes())
        .merge(crate::jobs::routes::routes())
        .merge(auth_routes::root_routes())
        .merge(permission_routes::routes())
        .merge(action_routes::routes())
        .merge(crate::users::routes::admin_routes())
        .merge(crate::rooms::routes::admin_routes())
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
        .filter_map(|r| std::future::ready(r.ok()))
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

/// Typed PMP connectivity summary (server status).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct PmpStatus {
    pub connected: bool,
    pub version: Option<String>,
    pub session_id: Option<String>,
}

/// Typed server status response (§22).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ServerStatusResponse {
    pub ppb_version: String,
    pub pmp: PmpStatus,
    pub db_configured: bool,
    pub metrics: serde_json::Value,
}

/// GET /api/v1/admin/server/status — scaffold summary.
#[utoipa::path(
    get,
    path = "/api/v1/admin/server/status",
    operation_id = "admin_server_status_get",
    responses(
        (status = 200, description = "server status", body = ServerStatusResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn server_status(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<ServerStatusResponse>, ApiError> {
    state
        .permissions
        .require(&state.db, &auth, "server:view")
        .await?;
    let openuds_state = state.openuds.state().await;
    Ok(Json(ServerStatusResponse {
        ppb_version: env!("CARGO_PKG_VERSION").to_string(),
        pmp: PmpStatus {
            connected: openuds_state.connected,
            version: openuds_state.server_version,
            session_id: openuds_state.session_id,
        },
        db_configured: state.db.is_some(),
        metrics: state.metrics.snapshot(),
    }))
}

