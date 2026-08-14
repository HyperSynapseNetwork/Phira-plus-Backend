//! Replay REST + WebSocket routes (design §12, contract §20 S-3).
//!
//! A Replay's identity is `(round_uuid, player_phira_id)`. Every path validates
//! access to the pair and pins the player server-side; the viewer WS can never
//! be redirected to another player's touches/judges. There is NO raw download.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
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
use crate::error::extractors::{ApiJson, ApiPath, ApiQuery};
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};
#[allow(unused_imports)]
use crate::openapi::{
    ReplayDetail, ReplayFramesResponse, ReplayJudgeFrame, ReplayListResponse,
    ReplayManifest, ReplaySummary, ReplayTouchPoint, ResolveShareResponse,
};
use crate::middleware::auth::OptionalAuthPrincipal;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/replays", get(list_replays))
        .route("/me/replays", get(list_my_replays))
        .route("/replays/share/{token}", get(resolve_share))
        .route("/replays/{round_uuid}", get(detail))
        .route("/replays/{round_uuid}/manifest", get(manifest))
        .route("/replays/{round_uuid}/frames", get(frames))
        .route("/replays/{round_uuid}/visibility", post(set_replay_visibility))
        .route("/replays/{round_uuid}/share", post(create_share))
        .route("/replays/{round_uuid}/share/{link_id}", delete(revoke_share))
}

#[derive(Debug, Deserialize)]
pub struct ReplayListParams {
    pub player_id: i32,
}

#[derive(Debug, Clone, Deserialize)]
struct ReplayRoundMeta {
    round_uuid: String,
    chart_id: i32,
    chart_name: String,
    room_id: String,
    #[serde(default)]
    players: Vec<i32>,
    started_at: i64,
    finished_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct OwnerReplayShareLink {
    pub id: Uuid,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct OwnerReplaySummary {
    pub round_uuid: String,
    pub player_phira_id: i64,
    pub chart_id: i32,
    pub chart_name: String,
    pub room_id: String,
    pub played_at: i64,
    pub finished_at: Option<i64>,
    pub visibility: String,
    pub share_links: Vec<OwnerReplayShareLink>,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct OwnerReplayListResponse {
    pub player_id: i64,
    pub items: Vec<OwnerReplaySummary>,
    pub total: i64,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ReplayCreatedShareLink {
    pub id: Uuid,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ReplayShareCreatedResponse {
    pub link: ReplayCreatedShareLink,
    /// Opaque raw token, returned once. Only a hash is persisted.
    pub token: String,
}

async fn round_meta(
    state: &AppState,
    round_uuid: &str,
    player_id: i32,
) -> Result<ReplayRoundMeta, ApiError> {
    let values = persist::fetch_rounds(&state.openuds, Some(round_uuid), Some(player_id), 1)
        .await
        .map_err(ApiError::from)?;
    values
        .into_iter()
        .find_map(|value| serde_json::from_value::<ReplayRoundMeta>(value).ok())
        .filter(|round| round.players.contains(&player_id))
        .ok_or_else(|| ApiError::new(ErrorCode::ReplayNotFound, "replay not found"))
}

/// GET /api/v1/me/replays — owner inventory including non-public visibility.
#[utoipa::path(
    get,
    path = "/api/v1/me/replays",
    operation_id = "me_replays_get",
    responses(
        (status = 200, description = "owner replay inventory", body = OwnerReplayListResponse),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn list_my_replays(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<OwnerReplayListResponse>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    let db = state.require_db()?;
    let user = crate::users::repo::find_by_id(db, auth.sub)
        .await?
        .ok_or_else(|| ApiError::new(crate::error::ErrorCode::UserNotFound, "user not found"))?;
    let player_id = user.phira_id;

    let overrides = sqlx::query_as::<_, (String, String)>(
        "SELECT pmp_replay_id, visibility FROM replay_overrides
         WHERE player_phira_id = $1 AND (owner_user_id = $2 OR owner_user_id IS NULL)",
    )
    .bind(player_id)
    .bind(auth.sub)
    .fetch_all(db)
    .await
    .map_err(super::db_err_public)?
    .into_iter()
    .collect::<std::collections::HashMap<_, _>>();

    let links = sqlx::query_as::<_, (Uuid, String, Option<DateTime<Utc>>, Option<DateTime<Utc>>)>(
        "SELECT id, replay_round, expires_at, revoked_at FROM replay_share_links
         WHERE player_phira_id = $1 AND created_by = $2 ORDER BY created_at DESC",
    )
    .bind(player_id)
    .bind(auth.sub)
    .fetch_all(db)
    .await
    .map_err(super::db_err_public)?;
    let mut links_by_round: std::collections::HashMap<String, Vec<OwnerReplayShareLink>> = std::collections::HashMap::new();
    for (id, replay_round, expires_at, revoked_at) in links {
        links_by_round.entry(replay_round).or_default().push(OwnerReplayShareLink { id, expires_at, revoked_at });
    }

    let rounds = persist::fetch_all_rounds(&state.openuds, None, Some(player_id as i32))
        .await
        .map_err(ApiError::from)?;
    let items = rounds
        .into_iter()
        .filter_map(|value| serde_json::from_value::<ReplayRoundMeta>(value).ok())
        .map(|round| OwnerReplaySummary {
            visibility: overrides.get(&round.round_uuid).cloned().unwrap_or_else(|| "public".to_string()),
            share_links: links_by_round.remove(&round.round_uuid).unwrap_or_default(),
            round_uuid: round.round_uuid,
            player_phira_id: player_id,
            chart_id: round.chart_id,
            chart_name: round.chart_name,
            room_id: round.room_id,
            played_at: round.started_at,
            finished_at: round.finished_at,
        })
        .collect::<Vec<_>>();
    let total = items.len() as i64;
    Ok(Json(OwnerReplayListResponse { player_id, items, total }))
}

/// GET /api/v1/replays?player_id=... — a player's durable round inventory.
///
/// Only `public` (incl. `inherit`→public) replays are listed; unlisted/private/
/// friends/custom overrides are never exposed in the public listing (contract §20).
#[utoipa::path(
    get,
    path = "/api/v1/replays",
    operation_id = "replays_get",
    params(("player_id" = i32, Query, description = "Phira player id")),
    responses(
        (status = 200, description = "public replay list", body = ReplayListResponse),
        (status = 502, description = "pmp unavailable", body = ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn list_replays(
    _auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<ReplayListParams>,
) -> Result<Json<ReplayListResponse>, ApiError> {
    // Listing is public; per-pair access is enforced on detail/manifest/WS.
    let db = state.require_db()?;
    // Replays with a non-public visibility override must not appear in lists.
    let non_public: std::collections::HashSet<String> = sqlx::query_as::<_, (String,)>(
        "SELECT pmp_replay_id FROM replay_overrides
         WHERE player_phira_id = $1 AND visibility NOT IN ('inherit', 'public')",
    )
    .bind(params.player_id)
    .fetch_all(db)
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "replay list visibility query failed");
        crate::error::ApiError::internal()
    })?
    .into_iter()
    .map(|(r,)| r)
    .collect();

    let rounds = persist::fetch_all_rounds(&state.openuds, None, Some(params.player_id))
        .await
        .map_err(ApiError::from)?;
    let items = rounds
        .into_iter()
        .filter_map(|value| serde_json::from_value::<ReplayRoundMeta>(value).ok())
        .filter(|round| !non_public.contains(&round.round_uuid))
        .map(|round| ReplaySummary {
            round_uuid: round.round_uuid,
            player_phira_id: params.player_id as i64,
            chart_id: round.chart_id,
            chart_name: round.chart_name,
            room_id: round.room_id,
            played_at: round.started_at,
            finished_at: round.finished_at,
            visibility: "public".to_string(),
        })
        .collect::<Vec<_>>();
    let total = items.len() as i64;
    Ok(Json(ReplayListResponse {
        player_id: params.player_id,
        items,
        total,
    }))
}

/// GET /api/v1/replays/share/{token} — resolve an opaque share token to the
/// pinned `(round_uuid, player_phira_id)` (S-3).
#[utoipa::path(
    get,
    path = "/api/v1/replays/share/{token}",
    operation_id = "replays_share_token_get",
    responses(
        (status = 200, description = "resolved replay identity", body = ResolveShareResponse),
        (status = 404, description = "invalid/expired/revoked token", body = ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn resolve_share(
    State(state): State<Arc<AppState>>,
    ApiPath(token): ApiPath<String>,
) -> Result<Json<ResolveShareResponse>, ApiError> {
    let db = state.require_db()?;
    let (round, player) = super::resolve_share_token(db, &token).await?;
    Ok(Json(ResolveShareResponse { round_uuid: round, player_phira_id: player }))
}

#[derive(Debug, Deserialize)]
pub struct ReplayDetailParams {
    pub player_id: i64,
    /// Optional share token for shared (e.g. unlisted) replays. When present it
    /// pins the `(round_uuid, player_phira_id)` pair; `player_id` is ignored.
    #[serde(default)]
    pub token: Option<String>,
}

/// GET /api/v1/replays/{round_uuid}?player_id=... — summary + visibility
/// (access-enforced for the pair).
#[utoipa::path(
    get,
    path = "/api/v1/replays/{round_uuid}",
    operation_id = "replays_round_uuid_get",
    params(
        ("round_uuid" = String, Path, description = "PMP round UUID"),
        ("player_id" = i64, Query, description = "Phira player id"),
        ("token" = Option<String>, Query, description = "Optional Replay share token"),
    ),
    responses(
        (status = 200, description = "replay detail", body = ReplayDetail),
        (status = 403, description = "access denied", body = ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn detail(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(round_uuid): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReplayDetailParams>,
) -> Result<Json<ReplayDetail>, ApiError> {
    let db = state.require_db()?;
    let Some(player) = resolve_replay_access(
        db,
        &round_uuid,
        params.player_id,
        auth.0.as_ref(),
        params.token.as_deref(),
    )
    .await?
    else {
        return Err(ApiError::permission_denied());
    };
    let visibility = super::visibility::effective_visibility(db, &round_uuid, player).await?;
    let meta = round_meta(&state, &round_uuid, player as i32).await?;
    let openuds = &state.openuds;
    let touches = persist::fetch_batches(openuds, "touches", 0, 1, Some(&round_uuid), Some(player as i32))
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_batches(openuds, "judges", 0, 1, Some(&round_uuid), Some(player as i32))
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ReplayDetail {
        round_uuid,
        player_phira_id: player,
        chart_id: meta.chart_id,
        chart_name: meta.chart_name,
        room_id: meta.room_id,
        started_at: meta.started_at,
        finished_at: meta.finished_at,
        visibility,
        touches: persist::summarize_batches(&persist::batches_of(&touches)),
        judges: persist::summarize_batches(&persist::batches_of(&judges)),
    }))
}

/// GET /api/v1/replays/{round_uuid}/manifest?player_id=... — frame counts /
/// players / range (access-enforced for the pair).
#[utoipa::path(
    get,
    path = "/api/v1/replays/{round_uuid}/manifest",
    operation_id = "replays_round_uuid_manifest_get",
    params(
        ("round_uuid" = String, Path, description = "PMP round UUID"),
        ("player_id" = i64, Query, description = "Phira player id"),
        ("token" = Option<String>, Query, description = "Optional Replay share token"),
    ),
    responses(
        (status = 200, description = "replay manifest", body = ReplayManifest),
        (status = 403, description = "access denied", body = ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn manifest(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(round_uuid): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReplayDetailParams>,
) -> Result<Json<ReplayManifest>, ApiError> {
    let db = state.require_db()?;
    let Some(player) = resolve_replay_access(
        db,
        &round_uuid,
        params.player_id,
        auth.0.as_ref(),
        params.token.as_deref(),
    )
    .await?
    else {
        return Err(ApiError::permission_denied());
    };
    let openuds = &state.openuds;
    let meta = round_meta(&state, &round_uuid, player as i32).await?;
    let touches = persist::fetch_all_batches(openuds, "touches", &round_uuid, player as i32)
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_all_batches(openuds, "judges", &round_uuid, player as i32)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ReplayManifest {
        round_uuid,
        player_phira_id: player,
        chart_id: meta.chart_id,
        chart_name: meta.chart_name,
        room_id: meta.room_id,
        started_at: meta.started_at,
        finished_at: meta.finished_at,
        touches: persist::summarize_batches(&touches),
        judges: persist::summarize_batches(&judges),
    }))
}

fn payload_items<T: serde::de::DeserializeOwned>(batches: Vec<Value>) -> Vec<T> {
    batches
        .into_iter()
        .filter_map(|batch| batch.get("payload").and_then(Value::as_array).cloned())
        .flatten()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect()
}

/// Full typed telemetry for a Replay viewer. Access is pinned to the same
/// `(round_uuid, player_phira_id)` policy as manifest and WebSocket routes.
#[utoipa::path(
    get,
    path = "/api/v1/replays/{round_uuid}/frames",
    operation_id = "replays_round_uuid_frames_get",
    params(
        ("round_uuid" = String, Path, description = "PMP round UUID"),
        ("player_id" = i64, Query, description = "Phira player id"),
        ("token" = Option<String>, Query, description = "Optional Replay share token"),
    ),
    responses(
        (status = 200, description = "typed Replay telemetry", body = ReplayFramesResponse),
        (status = 403, description = "access denied", body = ErrorEnvelope),
    ),
    tag = "replays"
)]
pub async fn frames(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(round_uuid): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReplayDetailParams>,
) -> Result<Json<ReplayFramesResponse>, ApiError> {
    let db = state.require_db()?;
    let Some(player) = resolve_replay_access(
        db,
        &round_uuid,
        params.player_id,
        auth.0.as_ref(),
        params.token.as_deref(),
    )
    .await?
    else {
        return Err(ApiError::permission_denied());
    };
    // Confirm that the pair belongs to a durable round before returning data.
    let _ = round_meta(&state, &round_uuid, player as i32).await?;
    let touches = persist::fetch_all_batches(&state.openuds, "touches", &round_uuid, player as i32)
        .await
        .map_err(ApiError::from)?;
    let judges = persist::fetch_all_batches(&state.openuds, "judges", &round_uuid, player as i32)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(ReplayFramesResponse {
        round_uuid,
        player_phira_id: player,
        touches: payload_items::<ReplayTouchPoint>(touches),
        judges: payload_items::<ReplayJudgeFrame>(judges),
    }))
}

const VISIBILITIES: &[&str] = &["inherit", "public", "friends", "private", "unlisted", "custom"];

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ReplayVisibilityResponse {
    pub r#override: ReplayOverride,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VisibilityBody {
    pub visibility: String,
}

/// POST /api/v1/replays/{round_uuid}/visibility?player_id=... — set visibility
/// for the pair (owner).
#[utoipa::path(
    post,
    path = "/api/v1/replays/{round_uuid}/visibility",
    operation_id = "replays_round_uuid_visibility_post",
    request_body = VisibilityBody,
    responses((status = 200, description = "visibility updated", body = ReplayVisibilityResponse)),
    tag = "replays"
)]
pub async fn set_replay_visibility(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(round_uuid): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReplayDetailParams>,
    ApiJson(body): ApiJson<VisibilityBody>,
) -> Result<Json<ReplayVisibilityResponse>, ApiError> {
    if !VISIBILITIES.contains(&body.visibility.as_str()) {
        return Err(ApiError::new(ErrorCode::ReplayVisibilityInvalid, "invalid replay visibility"));
    }
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, params.player_id, auth.sub).await?;
    let over = set_visibility(db, &round_uuid, params.player_id, auth.sub, &body.visibility).await?;
    Ok(Json(ReplayVisibilityResponse { r#override: over }))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ShareBody {
    pub expires_at: Option<DateTime<Utc>>,
}

/// POST /api/v1/replays/{round_uuid}/share?player_id=... — create a share link
/// for the pair (owner).
#[utoipa::path(
    post,
    path = "/api/v1/replays/{round_uuid}/share",
    operation_id = "replays_round_uuid_share_post",
    request_body = ShareBody,
    responses((status = 200, description = "share link created", body = ReplayShareCreatedResponse)),
    tag = "replays"
)]
pub async fn create_share(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(round_uuid): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReplayDetailParams>,
    ApiJson(body): ApiJson<ShareBody>,
) -> Result<Json<ReplayShareCreatedResponse>, ApiError> {
    let db = state.require_db()?;
    ensure_owner(db, &round_uuid, params.player_id, auth.sub).await?;
    let (link, token) = create_share_link(db, &round_uuid, params.player_id, auth.sub, body.expires_at).await?;
    Ok(Json(ReplayShareCreatedResponse { link: ReplayCreatedShareLink { id: link.id, expires_at: link.expires_at }, token }))
}

/// DELETE /api/v1/replays/{round_uuid}/share/{link_id}?player_id=... — revoke a
/// share link for the pair (owner).
#[utoipa::path(
    delete,
    path = "/api/v1/replays/{round_uuid}/share/{link_id}",
    operation_id = "replays_round_uuid_share_link_id_delete",
    responses((status = 204, description = "share link revoked"), (status = 404, description = "link not found", body = ErrorEnvelope)),
    tag = "replays"
)]
pub async fn revoke_share(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath((round_uuid, link_id)): ApiPath<(String, Uuid)>,
    ApiQuery(params): ApiQuery<ReplayDetailParams>,
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
    ApiPath(round_uuid): ApiPath<String>,
    ApiQuery(params): ApiQuery<ReplayWsParams>,
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
                return Err(ApiError::new(ErrorCode::ReplayPlayerRequired, "replay player_id required"));
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
