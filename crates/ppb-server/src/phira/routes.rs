//! Phira data proxy routes: `/api/v1/charts/*`, `/api/v1/records/*`, `/api/v1/users/*`.

use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::gateway::phira_gateway_error;
use crate::app::AppState;
use crate::middleware::auth::OptionalAuthPrincipal;
use crate::error::extractors::{ApiPath, ApiQuery};
use crate::error::ApiError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/charts", get(chart_list))
        .route("/charts/popular", get(chart_popular))
        .route("/charts/{id}", get(chart_detail))
        .route("/charts/{id}/preview", get(chart_preview))
        .route("/charts/{id}/viewer", get(chart_viewer))
        .route("/charts/{id}/records", get(chart_records))
        .route("/charts/{id}/top", get(chart_top))
        .route("/records", get(records_by_player))
        .route("/records/query/{chart_id}", get(records_query))
        .route("/records/list15/{chart_id}", get(records_list15))
        .route("/records/pool/{user_id}", get(records_pool))
        .route("/records/{id}", get(record_detail))
        .route("/users", get(users_search))
        .route("/users/{phira_id}", get(user_detail))
        .route("/users/{phira_id}/stats", get(user_stats))
}

#[derive(Debug, Deserialize)]
pub struct ChartListParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
    pub search: Option<String>,
    #[serde(rename = "type")]
    pub chart_type: Option<i64>,
    pub rating_min: Option<f64>,
    pub rating_max: Option<f64>,
    pub tags: Option<String>,
    pub order: Option<String>,
}

/// GET /api/v1/charts — chart list (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/charts",
    operation_id = "charts_get",
    responses(
        (status = 200, description = "chart list", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_list(
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<ChartListParams>,
) -> Result<Json<Value>, ApiError> {
    let mut result = state
        .phira_gateway
        .chart_list(
            params.page.unwrap_or(1),
            params.page_num.unwrap_or(20).min(30),
            params.search.as_deref(),
            params.chart_type,
            params.rating_min,
            params.rating_max,
            params.tags.as_deref(),
            params.order.as_deref(),
        )
        .await
        .map_err(phira_gateway_error)?;
    // Contract §18: chart list response always contains `total`.
    if result.get("total").is_none() {
        if let Some(map) = result.as_object_mut() {
            let total = map
                .get("results")
                .and_then(Value::as_array)
                .map(|a| a.len() as i64)
                .unwrap_or(0);
            map.insert("total".to_string(), serde_json::json!(total));
        }
    }
    Ok(Json(result))
}

/// GET /api/v1/charts/popular — popular charts.
#[utoipa::path(
    get,
    path = "/api/v1/charts/popular",
    operation_id = "charts_popular_get",
    responses(
        (status = 200, description = "popular charts", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_popular(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .chart_popular(30)
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/charts/{id} — chart detail.
#[utoipa::path(
    get,
    path = "/api/v1/charts/{id}",
    operation_id = "charts_id_get",
    responses(
        (status = 200, description = "chart detail", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_detail(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.chart(id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/charts/{id}/preview — chart file proxy fallback (design §12.7).
/// Preferred path is the browser fetching the Phira CDN file directly; this
/// route is used only when CORS blocks the direct download.
#[utoipa::path(
    get,
    path = "/api/v1/charts/{id}/preview",
    operation_id = "charts_id_preview_get",
    responses(
        (status = 200, description = "chart file bytes", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_preview(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<i64>,
) -> Result<axum::response::Response, ApiError> {
    let (bytes, content_type) = state
        .phira_gateway
        .fetch_chart_file(id)
        .await
        .map_err(phira_gateway_error)?;
    let mut resp = axum::response::Response::new(axum::body::Body::from(bytes));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_str(&content_type)
            .unwrap_or(axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=120"),
    );
    Ok(resp)
}

/// GET /api/v1/charts/{id}/viewer — bincode `(ChartInfo, Chart)` varint blob
/// (contract §19). Uses the same TTL-cached chart file as `/preview`.
#[utoipa::path(
    get,
    path = "/api/v1/charts/{id}/viewer",
    operation_id = "charts_id_viewer_get",
    responses(
        (status = 200, description = "bincode chart blob", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_viewer(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<i64>,
) -> Result<axum::response::Response, ApiError> {
    let bytes = state
        .phira_gateway
        .fetch_chart_file(id)
        .await
        .map_err(phira_gateway_error)?
        .0;
    let blob = crate::phira::viewer::build_chart_blob(&bytes)?;
    let mut resp = axum::response::Response::new(axum::body::Body::from(blob));
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=120"),
    );
    Ok(resp)
}

#[derive(Debug, Deserialize)]
pub struct RecordQueryParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// GET /api/v1/charts/{id}/records — chart records.
#[utoipa::path(
    get,
    path = "/api/v1/charts/{id}/records",
    operation_id = "charts_id_records_get",
    responses(
        (status = 200, description = "chart records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_records(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<i64>,
    ApiQuery(params): ApiQuery<RecordQueryParams>,
) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .record_query(id, params.page.unwrap_or(1), params.page_num.unwrap_or(20).min(30))
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/charts/{id}/top — chart top records.
#[utoipa::path(
    get,
    path = "/api/v1/charts/{id}/top",
    operation_id = "charts_id_top_get",
    responses(
        (status = 200, description = "top records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_top(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_list15(id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RecordByPlayerParams {
    pub player: i64,
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// GET /api/v1/records — records by player.
#[utoipa::path(
    get,
    path = "/api/v1/records",
    operation_id = "records_get",
    responses(
        (status = 200, description = "player records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_by_player(
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<RecordByPlayerParams>,
) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .record_query_player(params.player, params.page.unwrap_or(1), params.page_num.unwrap_or(20).min(30))
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/records/query/{chart_id} — chart records query.
#[utoipa::path(
    get,
    path = "/api/v1/records/query/{chart_id}",
    operation_id = "records_query_chart_id_get",
    responses(
        (status = 200, description = "chart records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_query(
    State(state): State<Arc<AppState>>,
    ApiPath(chart_id): ApiPath<i64>,
    ApiQuery(params): ApiQuery<RecordQueryParams>,
) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .record_query(chart_id, params.page.unwrap_or(1), params.page_num.unwrap_or(20).min(30))
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/records/list15/{chart_id} — chart top-15 records.
#[utoipa::path(
    get,
    path = "/api/v1/records/list15/{chart_id}",
    operation_id = "records_list15_chart_id_get",
    responses(
        (status = 200, description = "top-15 records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_list15(
    State(state): State<Arc<AppState>>,
    ApiPath(chart_id): ApiPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_list15(chart_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/records/pool/{user_id} — user record pool.
#[utoipa::path(
    get,
    path = "/api/v1/records/pool/{user_id}",
    operation_id = "records_pool_user_id_get",
    responses(
        (status = 200, description = "record pool", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_pool(
    State(state): State<Arc<AppState>>,
    ApiPath(user_id): ApiPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_get_pool(user_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/records/{id} — single record detail.
#[utoipa::path(
    get,
    path = "/api/v1/records/{id}",
    operation_id = "records_id_get",
    responses(
        (status = 200, description = "record detail", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn record_detail(
    State(state): State<Arc<AppState>>,
    ApiPath(id): ApiPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record(id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct UserSearchParams {
    pub search: String,
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// GET /api/v1/users — search Phira users.
#[utoipa::path(
    get,
    path = "/api/v1/users",
    operation_id = "users_get",
    responses(
        (status = 200, description = "user search results", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn users_search(
    State(state): State<Arc<AppState>>,
    ApiQuery(params): ApiQuery<UserSearchParams>,
) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .users_search(&params.search, params.page.unwrap_or(1), params.page_num.unwrap_or(20).min(30))
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PublicUserProfileResponse {
    pub phira_id: i64,
    pub username: String,
    pub avatar: Option<String>,
    pub bio: Option<String>,
    pub background_url: Option<String>,
    pub online_status: Option<String>,
    pub profile_visibility: String,
    pub rks: Option<f64>,
    pub stats: Option<Value>,
    pub friends_count: Option<i64>,
    pub is_friend: bool,
    pub is_blocked: bool,
}

/// GET /api/v1/users/{phira_id} — public community profile.
#[utoipa::path(
    get,
    path = "/api/v1/users/{phira_id}",
    operation_id = "users_phira_id_get",
    responses(
        (status = 200, description = "public community profile", body = PublicUserProfileResponse),
        (status = 404, description = "user not found", body = ErrorEnvelope),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn user_detail(
    auth: OptionalAuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiPath(phira_id): ApiPath<i64>,
) -> Result<Json<PublicUserProfileResponse>, ApiError> {
    let phira = state.phira_gateway.user(phira_id).await.map_err(phira_gateway_error)?;
    let username = phira.get("username").or_else(|| phira.get("name")).and_then(Value::as_str).unwrap_or("").to_string();
    if username.is_empty() {
        return Err(ApiError::new(crate::error::ErrorCode::UserNotFound, "user not found"));
    }
    let avatar = phira.get("avatar").and_then(Value::as_str).map(str::to_string);

    let Some(db) = state.db.as_ref() else {
        return Ok(Json(PublicUserProfileResponse {
            phira_id, username, avatar, bio: None, background_url: None, online_status: None, profile_visibility: "public".into(),
            rks: None, stats: None, friends_count: None, is_friend: false, is_blocked: false,
        }));
    };
    let local = crate::users::repo::find_by_phira_id(db, phira_id).await?;
    let Some(local) = local else {
        let stats = state.phira_gateway.user_stats(phira_id).await.ok();
        let rks = stats.as_ref().and_then(|v| v.get("rks")).and_then(Value::as_f64);
        return Ok(Json(PublicUserProfileResponse {
            phira_id, username, avatar, bio: None, background_url: None, online_status: None, profile_visibility: "public".into(),
            rks, stats, friends_count: None, is_friend: false, is_blocked: false,
        }));
    };
    let requester = auth.0.as_ref().filter(|principal| !principal.is_root()).map(|principal| principal.sub);
    let is_self = requester == Some(local.id);
    let is_friend = match requester { Some(id) if id != local.id => crate::social::are_friends(db, id, local.id).await?, _ => false };
    let is_blocked = match requester { Some(id) if id != local.id => crate::social::is_blocked(db, id, local.id).await?, _ => false };
    let profile = sqlx::query_as::<_, (Option<String>, Option<String>, String, bool, bool)>(
        "SELECT bio, background_url, profile_visibility, show_online_status, show_recent_activity FROM user_profiles WHERE user_id = $1",
    ).bind(local.id).fetch_optional(db).await.map_err(|error| { tracing::error!(%error, "public profile query failed"); ApiError::internal() })?;
    let (bio, background_url, visibility, show_online, _show_recent) = profile.unwrap_or((None, None, "public".into(), true, true));
    let can_view = is_self || visibility == "public" || (visibility == "friends" && is_friend);
    let stats = if can_view { state.phira_gateway.user_stats(phira_id).await.ok() } else { None };
    let rks = stats.as_ref().and_then(|v| v.get("rks")).and_then(Value::as_f64);
    let online_status = if can_view && show_online {
        state.player.info(phira_id as i32).await.ok().map(|p| {
            let online = p.get("online").and_then(Value::as_bool).unwrap_or_else(|| p.get("room_id").and_then(Value::as_str).is_some_and(|v| !v.is_empty()));
            if online { "online".to_string() } else { "offline".to_string() }
        })
    } else if can_view { Some("hidden".to_string()) } else { None };
    let friends_count = if can_view { crate::social::list_friends(db, local.id).await.ok().map(|items| items.len() as i64) } else { None };
    Ok(Json(PublicUserProfileResponse {
        phira_id, username, avatar, bio: if can_view { bio } else { None },
        background_url: if can_view { background_url } else { None },
        online_status, profile_visibility: visibility, rks, stats, friends_count, is_friend, is_blocked,
    }))
}

/// GET /api/v1/users/{phira_id}/stats — Phira user stats (rks).
#[utoipa::path(
    get,
    path = "/api/v1/users/{phira_id}/stats",
    operation_id = "users_phira_id_stats_get",
    responses(
        (status = 200, description = "user stats", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn user_stats(
    State(state): State<Arc<AppState>>,
    ApiPath(phira_id): ApiPath<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.user_stats(phira_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}
