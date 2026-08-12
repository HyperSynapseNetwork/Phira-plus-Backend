//! Replay REST + WebSocket routes (design §12, contract §20 S-3).
//!
//! A Replay's identity is `(round_uuid, player_phira_id)`. Every path validates
//! access to the pair and pins the player server-side; the viewer WS can never
//! be redirected to another player's touches/judges. There is NO raw download.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::persist;
use super::visibility::resolve_replay_access;
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
    pub player_id: i32,
}

/// GET /api/v1/replays?player_id=... — a player's distinct rounds (best-effort).
async fn list_replays(
    _auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ReplayListParams>,
) -> Result<Json<Value>, ApiError> {
    // Listing is public; per-pair access is enforced on detail/manifest/WS.
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
        "player_id": params.player_id,
        "replays": rounds.into_iter().collect::<Vec<String>>(),
        "total_frames": total_frames,
    })))
}

/// GET /api/v1/replays/share/{token} — resolve an opaque share token to the
/// pinned `(round_uuid, player_phira_id)` (S-3).
#[utoipa::path(
    get,
    path = "/api/v1/replays/share/{token}",
    responses(
        (status = 200, description = "resolved replay identity", body = serde_json::Value),
        (status = 404, description = "invalid/expired/revoked token", body = crate::error::ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn resolve_share(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    let (round, player) = super::resolve_share_token(db, &token).await?;
    Ok(Json(json!({ "round_uuid": round, "player_phira_id": player })))
}

#[derive(Debug, Deserialize)]
pub struct ReplayDetailParams {
    pub player_id: i64,
}

/// GET /api/v1/replays/{round_uuid}?player_id=... — summary + visibility
/// (access-enforced for the pair).
#[utoipa::path(
    get,
    path = "/api/v1/replays/{round_uuid}",
    responses(
        (status = 200, description = "replay detail", body = crate::openapi::ReplayDetail),
        (status = 403, description = "access denied", body = crate::error::ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn detail(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Query(params): Query<ReplayDetailParams>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    let Some(player) = resolve_replay_access(db, &round_uuid, params.player_id, auth.0.as_ref(), None).await?
    else {
        return Err(ApiError::permission_denied());
    };
    let visibility = super::visibility::effective_visibility(db, &round_uuid, player).await?;
    let openuds = &state.openuds;
    let touches = persist::fetch_batches(openuds, "touches", 0, 1, Some(&round_uuid), Some(player as i32))
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_batches(openuds, "judges", 0, 1, Some(&round_uuid), Some(player as i32))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "round_uuid": round_uuid,
        "player_phira_id": player,
        "visibility": visibility,
        "touches": persist::summarize_batches(&persist::batches_of(&touches)),
        "judges": persist::summarize_batches(&persist::batches_of(&judges)),
    })))
}

/// GET /api/v1/replays/{round_uuid}/manifest?player_id=... — frame counts /
/// players / range (access-enforced for the pair).
#[utoipa::path(
    get,
    path = "/api/v1/replays/{round_uuid}/manifest",
    responses(
        (status = 200, description = "replay manifest", body = crate::openapi::ReplayManifest),
        (status = 403, description = "access denied", body = crate::error::ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn manifest(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Query(params): Query<ReplayDetailParams>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    let Some(player) = resolve_replay_access(db, &round_uuid, params.player_id, auth.0.as_ref(), None).await?
    else {
        return Err(ApiError::permission_denied());
    };
    let openuds = &state.openuds;
    let touches = persist::fetch_batches(openuds, "touches", 0, persist::MAX_PAGE, Some(&round_uuid), Some(player as i32))
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_batches(openuds, "judges", 0, persist::MAX_PAGE, Some(&round_uuid), Some(player as i32))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({
        "round_uuid": round_uuid,
        "player_phira_id": player,
        "touches": persist::summarize_batches(&persist::batches_of(&touches)),
        "judges": persist::summarize_batches(&persist::batches_of(&judges)),
    })))
}

const VISIBILITIES: &[&str] = &["inherit", "public", "friends", "private", "unlisted", "custom"];

#[derive(Debug, Deserialize)]
pub struct VisibilityBody {
    pub visibility: String,
}

/// POST /api/v1/replays/{round_uuid}/visibility?player_id=... — set visibility
/// for the pair (owner).
async fn set_replay_visibility(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Query(params): Query<ReplayDetailParams>,
    Json(body): Json<VisibilityBody>,
) -> Result<Json<Value>, ApiError> {
    if !VISIBILITIES.contains(&body.visibility.as_str()) {
        return Err(ApiError::validation("visibility must be one of inherit|public|friends|private|unlisted|custom"));
    }
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, params.player_id, auth.sub).await?;
    let over = set_visibility(db, &round_uuid, params.player_id, auth.sub, &body.visibility).await?;
    Ok(Json(json!({ "override": over })))
}

#[derive(Debug, Deserialize)]
pub struct ShareBody {
    pub expires_at: Option<DateTime<Utc>>,
}

/// POST /api/v1/replays/{round_uuid}/share?player_id=... — create a share link
/// for the pair (owner).
async fn create_share(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Query(params): Query<ReplayDetailParams>,
    Json(body): Json<ShareBody>,
) -> Result<Json<Value>, ApiError> {
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, params.player_id, auth.sub).await?;
    let (link, token) = create_share_link(db, &round_uuid, params.player_id, auth.sub, body.expires_at).await?;
    Ok(Json(json!({ "link": link, "token": token })))
}

/// DELETE /api/v1/replays/{round_uuid}/share/{link_id}?player_id=... — revoke a
/// share link for the pair (owner).
async fn revoke_share(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path((round_uuid, link_id)): Path<(String, Uuid)>,
    Query(params): Query<ReplayDetailParams>,
) -> Result<StatusCode, ApiError> {
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, params.player_id, auth.sub).await?;
    revoke_share_link(db, link_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The caller may manage a `(round, player)` Replay if they own its override,
/// or (when no override exists yet) if they ARE the player — allowing the
/// player to create the first visibility/share.
async fn ensure_owner(
    db: &sqlx::PgPool,
    round_uuid: &str,
    player_phira_id: i64,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let row = sqlx::query_as::<_, ReplayOverride>(
        "SELECT id, pmp_replay_id, player_phira_id, owner_user_id, visibility, updated_at
         FROM replay_overrides WHERE pmp_replay_id = $1 AND player_phira_id = $2",
    )
    .bind(round_uuid)
    .bind(player_phira_id)
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
        None => {
            // No override yet: the caller must be the player themselves.
            let user = crate::users::repo::find_by_id(db, user_id).await?;
            if user.map(|u| u.phira_id) == Some(player_phira_id) {
                Ok(())
            } else {
                Err(ApiError::permission_denied())
            }
        }
    }
}

// ── WebSocket viewer stream ────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ReplayWsParams {
    #[serde(default)]
    pub token: Option<String>,
    /// Target player when no share token; server pins the resolved player.
    #[serde(default)]
    pub player_id: Option<i64>,
}

/// WSS /ws/v1/replays/{round_uuid}?token=...&player_id=... — paged viewer stream.
/// After auth the `(round_uuid, player_phira_id)` pair is pinned; the client's
/// per-fetch `player_id` cannot override it (S-3).
pub async fn replay_ws(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(round_uuid): Path<String>,
    Query(params): Query<ReplayWsParams>,
    ws: WebSocketUpgrade,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    // Determine the requested player (token pins it; otherwise the query
    // player_id, or the requester's own phira_id when authenticated).
    let requested_player = if params.token.is_some() {
        params.player_id.unwrap_or(0)
    } else {
        match (params.player_id, &auth.0) {
            (Some(p), _) => p,
            (None, Some(principal)) => {
                let user = crate::users::repo::find_by_id(db, principal.sub).await?;
                user.map(|u| u.phira_id).unwrap_or(0)
            }
            (None, None) => {
                return Err(ApiError::validation("player_id required"));
            }
        }
    };
    let Some(pinned) = resolve_replay_access(
        db,
        &round_uuid,
        requested_player,
        auth.0.as_ref(),
        params.token.as_deref(),
    )
    .await?
    else {
        return Err(ApiError::permission_denied());
    };
    Ok(ws.on_upgrade(move |socket| replay_ws_task(socket, state, round_uuid, pinned as i32)))
}

async fn replay_ws_task(
    socket: WebSocket,
    state: Arc<AppState>,
    round_uuid: String,
    pinned_player: i32,
) {
    use futures_util::{SinkExt, StreamExt};
    let (mut sink, mut stream) = socket.split();
    let openuds = state.openuds.clone();
    let mut since = 0i64;
    while let Some(msg) = stream.next().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            _ => continue,
        };
        let parsed: Value = match serde_json::from_str(text.as_str()) {
            Ok(v) => v,
            Err(_) => {
                let _ = sink.send(Message::text(json!({"type":"error","message":"invalid json"}).to_string())).await;
                continue;
            }
        };
        let typ = parsed.get("type").and_then(Value::as_str).unwrap_or("");
        if typ == "close" {
            break;
        }
        if typ != "fetch" {
            let _ = sink.send(Message::text(json!({"type":"error","message":"expected fetch"}).to_string())).await;
            continue;
        }
        let stream = parsed.get("stream").and_then(Value::as_str).unwrap_or("touches").to_string();
        if !matches!(stream.as_str(), "touches" | "judges") {
            let _ = sink.send(Message::text(json!({"type":"error","message":"stream must be touches|judges"}).to_string())).await;
            continue;
        }
        since = parsed.get("since").and_then(Value::as_i64).unwrap_or(since);
        // Pinned player is fixed; client player_id is ignored.
        match persist::fetch_batches(&openuds, &stream, since, persist::MAX_PAGE, Some(&round_uuid), Some(pinned_player)).await {
            Ok(v) => {
                let batches = persist::batches_of(&v);
                let last_seq = batches
                    .iter()
                    .filter_map(|b| b.get("sequence").and_then(Value::as_i64))
                    .max()
                    .unwrap_or(since);
                let done = batches.is_empty();
                let _ = sink
                    .send(Message::text(
                        json!({
                            "type": "batches",
                            "stream": stream,
                            "batches": batches,
                            "last_sequence": last_seq,
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
                let _ = sink.send(Message::text(json!({"type":"error","message":e.to_string()}).to_string())).await;
                break;
            }
        }
    }
}
