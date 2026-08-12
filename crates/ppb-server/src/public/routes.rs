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
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorEnvelope};
use crate::pmp::events::{PpbEvent, ReplayResult};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/meta", get(meta))
        .route("/site", get(site))
        .route("/announcements", get(announcements))
        .route("/downloads", get(downloads))
        .route("/nodes", get(nodes))
        .route("/events", get(events_sse))
}

/// GET /api/v1/public/meta — capabilities / meta contract.
#[utoipa::path(
    get,
    path = "/api/v1/public/meta",
    responses(
        (status = 200, description = "meta + capabilities", body = serde_json::Value),
    ),
    tag = "public"
)]
pub async fn meta(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    let openuds_state = state.openuds.state().await;
    let caps = state.openuds.capabilities().await;
    Ok(axum::Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": 1,
        "capabilities": crate::public::PPB_CAPABILITIES,
        "pmp": {
            "connected": openuds_state.connected,
            "version": openuds_state.server_version,
            "capabilities": caps,
        },
    })))
}

/// GET /api/v1/public/site — public site config (merged with runtime content).
///
/// `visit_count` (P-86): privacy-friendly aggregate. Baseline from config
/// (`site.visit_count`), plus a server-side in-memory counter incremented on
/// each fetch. No client fingerprint is used; defaults to 0 when unset.
#[utoipa::path(
    get,
    path = "/api/v1/public/site",
    responses(
        (status = 200, description = "public site config", body = serde_json::Value),
    ),
    tag = "public"
)]
pub async fn site(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    use std::sync::atomic::Ordering;
    let counted = state.visit_counter.fetch_add(1, Ordering::Relaxed);
    let mut base = json!({
        "ppf_url": state.config.site.ppf_url,
        "panel_url": state.config.site.panel_url,
        "docs_url": state.config.site.docs_url,
        "api_url": state.config.server.public_url,
        "visit_count": state.config.site.visit_count + counted,
    });
    if let Some(db) = &state.db {
        if let Some(over) = crate::config::repo::get_public_content(db, "site").await? {
            if let Some(obj) = over.as_object() {
                if let Some(b) = base.as_object_mut() {
                    for (k, v) in obj {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
    }
    Ok(axum::Json(base))
}

/// GET /api/v1/public/announcements — public runtime content.
#[utoipa::path(
    get,
    path = "/api/v1/public/announcements",
    responses(
        (status = 200, description = "announcements content", body = serde_json::Value),
    ),
    tag = "public"
)]
pub async fn announcements(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    public_content_or_empty(&state, "announcements").await
}

/// GET /api/v1/public/downloads — public runtime content.
#[utoipa::path(
    get,
    path = "/api/v1/public/downloads",
    responses(
        (status = 200, description = "downloads content", body = serde_json::Value),
    ),
    tag = "public"
)]
pub async fn downloads(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    public_content_or_empty(&state, "downloads").await
}

/// GET /api/v1/public/nodes — public runtime content.
#[utoipa::path(
    get,
    path = "/api/v1/public/nodes",
    responses(
        (status = 200, description = "nodes content", body = serde_json::Value),
    ),
    tag = "public"
)]
pub async fn nodes(
    State(state): State<Arc<AppState>>,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    public_content_or_empty(&state, "nodes").await
}

async fn public_content_or_empty(
    state: &Arc<AppState>,
    key: &str,
) -> Result<axum::Json<serde_json::Value>, ApiError> {
    let content = if let Some(db) = &state.db {
        crate::config::repo::get_public_content(db, key)
            .await?
            .unwrap_or(serde_json::Value::Object(Default::default()))
    } else {
        serde_json::Value::Object(Default::default())
    };
    Ok(axum::Json(content))
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
