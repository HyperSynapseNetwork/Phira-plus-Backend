//! `/api/v1/admin/logs/*` routes (Panel §18.11).

use std::convert::Infallible;
use std::sync::Arc;

use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;

use super::translator::{translate, translate_pattern, TranslatedError};
use crate::app::AppState;
use crate::auth::reauth::ReauthRisk;
use crate::auth::routes::check_reauth_header;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};
use crate::pmp::openuds::client::OpenUdsError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/logs", get(history))
        .route("/logs/stream", get(stream))
        .route("/logs/input", get(input).post(submit_input))
        .route("/logs/translate", get(translate_endpoint).post(translate_post))
}

#[derive(Debug, Deserialize)]
pub struct LogParams {
    pub limit: Option<u64>,
}

/// GET /api/v1/admin/logs — recent PMP log history.
#[utoipa::path(
    get,
    path = "/api/v1/admin/logs",
    operation_id = "admin_logs_get",
    responses(
        (status = 200, description = "log history", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn history(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<LogParams>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    let result = state
        .openuds
        .command("logs.history", json!({ "limit": params.limit.unwrap_or(200) }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

async fn input(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<LogParams>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "pmp:cli").await?;
    let result = state
        .openuds
        .command("logs.input", json!({ "limit": params.limit.unwrap_or(200) }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct LogInputBody {
    pub command: String,
}

/// POST /api/v1/admin/logs/input — submit a PMP console command (full audit).
/// Requires an elevated reauth context; audit records the FINAL result.
#[utoipa::path(
    post,
    path = "/api/v1/admin/logs/input",
    operation_id = "admin_logs_input_post",
    request_body = LogInputBody,
    responses(
        (status = 200, description = "command result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn submit_input(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<LogInputBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "pmp:cli").await?;
    state
        .rate_limiter
        .check(&format!("raw-cli:{}", auth.sub), state.config.rate_limit.raw_cli_per_minute)?;
    // Gate 0 A3: raw console requires an elevated reauth context.
    check_reauth_header(&state, &auth, &headers, ReauthRisk::High)?;

    let (result, result_status, error_code) = match tokio::time::timeout(
        Duration::from_secs(30),
        crate::pmp::cli::cli_execute(&state.openuds, &body.command),
    )
    .await
    {
        Ok(Ok(v)) => (Ok(v), "succeeded", String::new()),
        Ok(Err(e)) => (Err(ApiError::from(e)), "failed", "cli_execute_error".to_string()),
        Err(_) => (
            Err(ApiError::new(ErrorCode::PmpUnavailable, "command timed out")),
            "timeout",
            "timeout".to_string(),
        ),
    };
    if let Some(db) = &state.db {
        let _ = crate::audit::service::record_principal(
            db,
            &auth,
            "pmp.cli.execute",
            "pmp",
            "console",
            serde_json::json!({ "command": "[REDACTED input]" }),
            result_status,
            &error_code,
            "",
            "",
            "",
        )
        .await;
    }
    Ok(Json(result?))
}

/// GET /api/v1/admin/logs/stream — live PMP logs via OpenUDS `logs` stream.
async fn stream(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    if !state.openuds.state().await.connected {
        return Err(ApiError::new(
            crate::error::ErrorCode::PmpUnavailable,
            "pmp not connected",
        ));
    }
    // Best-effort (re)subscribe to the logs stream on this connection.
    let _ = state.openuds.subscribe_stream("logs").await;

    let rx = state.openuds.subscribe_stream_frames();
    let stream = BroadcastStream::new(rx)
        .filter_map(|r| std::future::ready(r.ok()))
        .filter_map(|f| std::future::ready((f.stream == "logs").then_some(f)))
        .map(|f| {
            Ok::<_, Infallible>(
                Event::default()
                    .event("log")
                    .data(serde_json::to_string(&f.frames).unwrap_or_else(|_| "{}".to_string())),
            )
        });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    ))
}

/// Typed translation response (§23 `{code, translated}`).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct TranslateResponse {
    pub code: String,
    pub translated: Option<TranslatedError>,
}

/// GET /api/v1/admin/logs/translate?code=... — rule-based error translation.
#[utoipa::path(
    get,
    path = "/api/v1/admin/logs/translate",
    operation_id = "admin_logs_translate_get",
    responses(
        (status = 200, description = "translated log message", body = TranslateResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn translate_endpoint(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<TranslateParams>,
) -> Result<Json<TranslateResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    let code = params.code();
    let translated = translate(&code).or_else(|| translate_pattern(&code));
    Ok(Json(TranslateResponse { code, translated }))
}

/// §23 P-91 translate request: Panel sends `{ code }`; `{ error_code }` is
/// accepted and normalized to `code` for backward compatibility.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TranslateParams {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
}

impl TranslateParams {
    /// Normalize `code` / `error_code` into a single code string.
    pub fn code(&self) -> String {
        self.code
            .clone()
            .or_else(|| self.error_code.clone())
            .unwrap_or_default()
    }
}

/// POST /api/v1/admin/logs/translate — body-based error translation (contract §17).
#[utoipa::path(
    post,
    path = "/api/v1/admin/logs/translate",
    operation_id = "admin_logs_translate_post",
    request_body = TranslateParams,
    responses(
        (status = 200, description = "translated log message", body = TranslateResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn translate_post(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<TranslateParams>,
) -> Result<Json<TranslateResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    let code = body.code();
    let translated = translate(&code).or_else(|| translate_pattern(&code));
    Ok(Json(TranslateResponse { code, translated }))
}

fn map_err(e: OpenUdsError) -> ApiError {
    ApiError::from(e)
}
