//! `/api/v1/admin/logs/*` routes (Panel §18.11).

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::stream::Stream;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;

use super::translator::{translate, translate_pattern};
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;
use crate::pmp::openuds::client::OpenUdsError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/logs", get(history))
        .route("/logs/stream", get(stream))
        .route("/logs/input", get(input).post(submit_input))
        .route("/logs/translate", get(translate_endpoint))
}

#[derive(Debug, Deserialize)]
pub struct LogParams {
    pub limit: Option<u64>,
}

async fn history(
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

#[derive(Debug, Deserialize)]
pub struct LogInputBody {
    pub command: String,
}

/// POST /api/v1/admin/logs/input — submit a PMP console command (full audit).
async fn submit_input(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<LogInputBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "pmp:cli").await?;
    state
        .rate_limiter
        .check(&format!("raw-cli:{}", auth.sub), state.config.rate_limit.raw_cli_per_minute)?;
    let result = crate::pmp::cli::cli_execute(&state.openuds, &body.command)
        .await
        .map_err(map_err)?;
    if let Some(db) = &state.db {
        crate::audit::service::record_principal(
            db,
            &auth,
            "pmp.cli.execute",
            "pmp",
            "console",
            serde_json::json!({ "command": "[REDACTED input]" }),
            "success",
            "",
            "",
            "",
            "",
        )
        .await?;
    }
    Ok(Json(result))
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

/// GET /api/v1/admin/logs/translate?code=... — rule-based error translation.
pub async fn translate_endpoint(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<TranslateParams>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "server:view").await?;
    let t = translate(&params.code).or_else(|| translate_pattern(&params.code));
    Ok(Json(json!({ "code": params.code, "translated": t })))
}

#[derive(Debug, Deserialize)]
pub struct TranslateParams {
    pub code: String,
}

fn map_err(e: OpenUdsError) -> ApiError {
    ApiError::from(e)
}
