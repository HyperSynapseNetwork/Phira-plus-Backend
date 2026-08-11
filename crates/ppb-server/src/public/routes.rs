//! Public routes: meta, site, SSE events.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use futures_util::stream::{self, Stream};
use futures_util::StreamExt;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;
use crate::pmp::events::{PpbEvent, ReplayResult};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/meta", get(meta))
        .route("/site", get(site))
        .route("/events", get(events_sse))
}

/// GET /api/v1/public/meta — capabilities / meta contract.
pub async fn meta(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    let openuds_state = state.openuds.state().await;
    let caps = state.openuds.capabilities().await;
    Ok(axum::Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": 1,
        "capabilities": [
            "rooms.v1",
            "replay.persist.v1",
            "room.chat.v1",
            "notifications.actions.v1",
        ],
        "pmp": {
            "connected": openuds_state.connected,
            "version": openuds_state.server_version,
            "capabilities": caps,
        },
    })))
}

/// GET /api/v1/public/site — public site config (no secrets).
pub async fn site(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    Ok(axum::Json(json!({
        "ppf_url": state.config.site.ppf_url,
        "panel_url": state.config.site.panel_url,
        "docs_url": state.config.site.docs_url,
        "api_url": state.config.server.public_url,
    })))
}

/// SSE stream for authenticated clients.
///
/// Envelope: `{id, type, version, occurred_at, resource, data}`. Supports
/// `Last-Event-ID` replay (or snapshot+realtime fallback) + heartbeat.
pub async fn events_sse(
    State(state): State<Arc<AppState>>,
    _auth: AuthPrincipal,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last_event_id = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let rx = state.events.subscribe();

    // Replay from Last-Event-ID, or snapshot fallback.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_shape() {
        let v = json!({
            "version": "0.1.0",
            "api_version": 1,
            "capabilities": ["rooms.v1"],
            "pmp": {"connected": false, "version": null, "capabilities": []},
        });
        assert_eq!(v["api_version"], 1);
        assert!(v["pmp"].is_object());
    }
}
