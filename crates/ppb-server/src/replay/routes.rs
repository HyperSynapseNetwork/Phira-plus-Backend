//! Replay REST + WebSocket routes (design §12, contract §4/§10/§13).
//!
//! `GET /api/v1/replays/{round_uuid}` and `/manifest` (REST), plus
//! `WSS /ws/v1/replays/{round_uuid}` (paged viewer stream via persist.touches/
//! judges). Visibility is enforced on every path; there is NO raw download.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::persist;
use super::visibility::check_replay_access;
use super::{create_share_link, revoke_share_link, set_visibility, ReplayOverride};
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;
use crate::middleware::auth::OptionalAuthPrincipal;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/replays", get(list_replays))
        .route("/replays/share/{token}", get(resolve_share))
        .route("/replays/{round_uuid}", get(detail))
        .route("/replays/{round_uuid}/manifest", get(manifest))
        .route("/replays/{round_uuid}/visibility", post(set_replay_visibility))
        .route("/replays/{round_uuid}/share", post(create_share))
        .route("/replays/{round_uuid}/share/{link_id}", delete(revoke_share))
}

#[derive(Debug, Deserialize)]
pub struct ReplayListParams {
    #[serde(rename = "playerId")]
    pub player_id: i32,
}

/// GET /api/v1/replays?playerId=... — a player's distinct rounds (best-effort).
async fn list_replays(
    _auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReplayListParams>,
) -> Result<Json<Value>, ApiError> {
    // Listing is public; per-round access is enforced on detail/manifest/WS.
    let openuds = &state.openuds;
    let mut rounds: BTreeSet<String> = BTreeSet::new();
    let mut total_frames = 0i64;
    let mut since = 0i64;
    for _ in 0..50 {
        let v = persist::fetch_batches(openuds, "touches", since, persist::MAX_PAGE, None, Some(params.player_id))
            .await
            .map_err(ApiError::from)?;
        let batches = persist::batches_of(&v);
        if batches.is_empty() {
            break;
        }
        for b in &batches {
            if let Some(r) = b.get("round_uuid").and_then(Value::as_str) {
                rounds.insert(r.to_string());
            }
            if let Some(c) = b.get("count").and_then(Value::as_i64) {
                total_frames += c;
            }
        }
        let last = batches
            .iter()
            .filter_map(|b| b.get("sequence").and_then(Value::as_i64))
            .max()
            .unwrap_or(since);
        if last <= since {
            break;
        }
        since = last;
    }
    Ok(Json(json!({
        "playerId": params.player_id,
        "replays": rounds.into_iter().collect::<Vec<String>>(),
        "totalFrames": total_frames,
    })))
}

/// GET /api/v1/replays/share/{token} — resolve an opaque share token to a round.
async fn resolve_share(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    let round = super::resolve_share_token(db, &token).await?;
    Ok(Json(json!({ "round_uuid": round })))
}

/// GET /api/v1/replays/{round_uuid} — summary + visibility (access-enforced).
async fn detail(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    let allowed = check_replay_access(db, &round_uuid, auth.0.as_ref(), None).await?;
    if !allowed {
        return Err(ApiError::permission_denied());
    }
    let visibility = super::visibility::effective_visibility(db, &round_uuid).await?;
    let openuds = &state.openuds;
    let touches = persist::fetch_batches(openuds, "touches", 0, 1, Some(&round_uuid), None)
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_batches(openuds, "judges", 0, 1, Some(&round_uuid), None)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "roundUuid": round_uuid,
        "visibility": visibility,
        "touches": persist::summarize_batches(&persist::batches_of(&touches)),
        "judges": persist::summarize_batches(&persist::batches_of(&judges)),
    })))
}

/// GET /api/v1/replays/{round_uuid}/manifest — frame counts / players / range.
async fn manifest(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    let allowed = check_replay_access(db, &round_uuid, auth.0.as_ref(), None).await?;
    if !allowed {
        return Err(ApiError::permission_denied());
    }
    let openuds = &state.openuds;
    let touches = persist::fetch_batches(openuds, "touches", 0, persist::MAX_PAGE, Some(&round_uuid), None)
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_batches(openuds, "judges", 0, persist::MAX_PAGE, Some(&round_uuid), None)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "roundUuid": round_uuid,
        "touches": persist::summarize_batches(&persist::batches_of(&touches)),
        "judges": persist::summarize_batches(&persist::batches_of(&judges)),
    })))
}

const VISIBILITIES: &[&str] = &["inherit", "public", "friends", "private", "unlisted", "custom"];

#[derive(Debug, Deserialize)]
pub struct VisibilityBody {
    pub visibility: String,
}

/// POST /api/v1/replays/{round_uuid}/visibility — set visibility (owner).
async fn set_replay_visibility(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Json(body): Json<VisibilityBody>,
) -> Result<Json<Value>, ApiError> {
    if !VISIBILITIES.contains(&body.visibility.as_str()) {
        return Err(ApiError::validation("visibility must be one of inherit|public|friends|private|unlisted|custom"));
    }
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, auth.sub).await?;
    let over = set_visibility(db, &round_uuid, auth.sub, &body.visibility).await?;
    Ok(Json(json!({ "override": over })))
}

#[derive(Debug, Deserialize)]
pub struct ShareBody {
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// POST /api/v1/replays/{round_uuid}/share — create a share link (owner).
async fn create_share(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Json(body): Json<ShareBody>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, auth.sub).await?;
    let (link, token) = create_share_link(db, &round_uuid, auth.sub, body.expires_at).await?;
    Ok(Json(json!({ "link": link, "token": token })))
}

/// DELETE /api/v1/replays/{round_uuid}/share/{link_id} — revoke a share link.
async fn revoke_share(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path((round_uuid, link_id)): Path<(String, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, auth.sub).await?;
    revoke_share_link(db, link_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn ensure_owner(db: &sqlx::PgPool, round_uuid: &str, user_id: Uuid) -> Result<(), ApiError> {
    let row = sqlx::query_as::<_, ReplayOverride>(
        "SELECT id, pmp_replay_id, owner_user_id, visibility, updated_at
         FROM replay_overrides WHERE pmp_replay_id = $1",
    )
    .bind(round_uuid)
    .fetch_optional(db)
    .await
    .map_err(super::db_err_public)?;
    match row {
        Some(o) => {
            if o.owner_user_id != Some(user_id) {
                return Err(ApiError::permission_denied());
            }
            Ok(())
        }
        None => Err(ApiError::not_found("replay override")),
    }
}

// ── WebSocket viewer stream ────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ReplayWsParams {
    #[serde(default)]
    pub token: Option<String>,
}

/// WSS /ws/v1/replays/{round_uuid} — paged viewer stream.
pub async fn replay_ws(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Query(params): Query<ReplayWsParams>,
    ws: WebSocketUpgrade,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    let allowed = check_replay_access(db, &round_uuid, auth.0.as_ref(), params.token.as_deref()).await?;
    if !allowed {
        return Err(ApiError::permission_denied());
    }
    Ok(ws.on_upgrade(move |socket| replay_ws_task(socket, state, round_uuid)))
}

async fn replay_ws_task(socket: WebSocket, state: Arc<AppState>, round_uuid: String) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sink, mut stream) = socket.split();
    let openuds = state.openuds.clone();
    let mut since = 0i64;
    let mut player: Option<i32> = None;
    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };
        let parsed: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => {
                let _ = sink.send(Message::Text(json!({"type":"error","message":"invalid json"}).to_string())).await;
                continue;
            }
        };
        let typ = parsed.get("type").and_then(Value::as_str).unwrap_or("");
        if typ == "close" {
            break;
        }
        if typ != "fetch" {
            let _ = sink.send(Message::Text(json!({"type":"error","message":"expected fetch"}).to_string())).await;
            continue;
        }
        let stream = parsed.get("stream").and_then(Value::as_str).unwrap_or("touches").to_string();
        if !matches!(stream.as_str(), "touches" | "judges") {
            let _ = sink.send(Message::Text(json!({"type":"error","message":"stream must be touches|judges"}).to_string())).await;
            continue;
        }
        since = parsed.get("since").and_then(Value::as_i64).unwrap_or(since);
        if let Some(pid) = parsed.get("player_id").and_then(Value::as_i64) {
            player = Some(pid as i32);
        }
        match persist::fetch_batches(&openuds, &stream, since, persist::MAX_PAGE, Some(&round_uuid), player).await {
            Ok(v) => {
                let batches = persist::batches_of(&v);
                let last_seq = batches
                    .iter()
                    .filter_map(|b| b.get("sequence").and_then(Value::as_i64))
                    .max()
                    .unwrap_or(since);
                let done = batches.is_empty();
                let _ = sink
                    .send(Message::Text(
                        json!({
                            "type": "batches",
                            "stream": stream,
                            "batches": batches,
                            "lastSequence": last_seq,
                            "done": done,
                        })
                        .to_string(),
                    ))
                    .await;
                since = last_seq;
                if done {
                    break;
                }
            }
            Err(e) => {
                let _ = sink.send(Message::Text(json!({"type":"error","message":e.to_string()}).to_string())).await;
                break;
            }
        }
    }
}
