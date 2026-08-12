//! Phira data proxy routes: `/api/v1/charts/*`, `/api/v1/records/*`, `/api/v1/users/*`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use super::gateway::phira_gateway_error;
use crate::app::AppState;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorEnvelope};

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
}

/// GET /api/v1/charts — chart list (paginated).
#[utoipa::path(
    get,
    path = "/api/v1/charts",
    responses(
        (status = 200, description = "chart list", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ChartListParams>,
) -> Result<Json<Value>, ApiError> {
    let mut result = state
        .phira_gateway
        .chart_list(
            params.page.unwrap_or(1),
            params.page_num.unwrap_or(20).min(30),
            params.search.as_deref(),
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
    responses(
        (status = 200, description = "chart detail", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
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
    responses(
        (status = 200, description = "chart file bytes", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
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
    responses(
        (status = 200, description = "bincode chart blob", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_viewer(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
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
    responses(
        (status = 200, description = "chart records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_records(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Query(params): Query<RecordQueryParams>,
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
    responses(
        (status = 200, description = "top records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn chart_top(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
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
    responses(
        (status = 200, description = "player records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_by_player(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RecordByPlayerParams>,
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
    responses(
        (status = 200, description = "chart records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_query(
    State(state): State<Arc<AppState>>,
    Path(chart_id): Path<i64>,
    Query(params): Query<RecordQueryParams>,
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
    responses(
        (status = 200, description = "top-15 records", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_list15(
    State(state): State<Arc<AppState>>,
    Path(chart_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_list15(chart_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/records/pool/{user_id} — user record pool.
#[utoipa::path(
    get,
    path = "/api/v1/records/pool/{user_id}",
    responses(
        (status = 200, description = "record pool", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn records_pool(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_get_pool(user_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/records/{id} — single record detail.
#[utoipa::path(
    get,
    path = "/api/v1/records/{id}",
    responses(
        (status = 200, description = "record detail", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn record_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
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
    responses(
        (status = 200, description = "user search results", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn users_search(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UserSearchParams>,
) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .users_search(&params.search, params.page.unwrap_or(1), params.page_num.unwrap_or(20).min(30))
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/users/{phira_id} — Phira user profile.
#[utoipa::path(
    get,
    path = "/api/v1/users/{phira_id}",
    responses(
        (status = 200, description = "user profile", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn user_detail(
    State(state): State<Arc<AppState>>,
    Path(phira_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.user(phira_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

/// GET /api/v1/users/{phira_id}/stats — Phira user stats (rks).
#[utoipa::path(
    get,
    path = "/api/v1/users/{phira_id}/stats",
    responses(
        (status = 200, description = "user stats", body = serde_json::Value),
        (status = 502, description = "phira unavailable", body = ErrorEnvelope),
    ),
    tag = "phira"
)]
pub async fn user_stats(
    State(state): State<Arc<AppState>>,
    Path(phira_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.user_stats(phira_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}
