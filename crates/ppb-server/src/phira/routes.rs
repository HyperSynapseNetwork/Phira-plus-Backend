//! Phira data proxy routes: `/api/v1/charts/*`, `/api/v1/records/*`, `/api/v1/users/*`.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use super::gateway::phira_gateway_error;
use crate::app::AppState;
use crate::error::ApiError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/charts", get(chart_list))
        .route("/charts/popular", get(chart_popular))
        .route("/charts/{id}", get(chart_detail))
        .route("/charts/{id}/records", get(chart_records))
        .route("/charts/{id}/top", get(chart_top))
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

async fn chart_list(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ChartListParams>,
) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .chart_list(
            params.page.unwrap_or(1),
            params.page_num.unwrap_or(20).min(30),
            params.search.as_deref(),
        )
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

async fn chart_popular(State(state): State<Arc<AppState>>) -> Result<Json<Value>, ApiError> {
    let result = state
        .phira_gateway
        .chart_popular(30)
        .await
        .map_err(phira_gateway_error)?;
    Ok(Json(result))
}

async fn chart_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.chart(id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
pub struct RecordQueryParams {
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

async fn chart_records(
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

async fn chart_top(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_list15(id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

async fn records_query(
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

async fn records_list15(
    State(state): State<Arc<AppState>>,
    Path(chart_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_list15(chart_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

async fn records_pool(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.record_get_pool(user_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

async fn record_detail(
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

async fn users_search(
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

async fn user_detail(
    State(state): State<Arc<AppState>>,
    Path(phira_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.user(phira_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}

async fn user_stats(
    State(state): State<Arc<AppState>>,
    Path(phira_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let result = state.phira_gateway.user_stats(phira_id).await.map_err(phira_gateway_error)?;
    Ok(Json(result))
}
