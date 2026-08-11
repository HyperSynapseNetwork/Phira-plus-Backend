//! `/api/v1/admin/plugins/*` routes (PMP plugin.*).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;
use crate::pmp::openuds::client::OpenUdsError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/plugins", get(list).post(reload))
        .route("/plugins/{name}", get(info).delete(remove))
        .route("/plugins/{name}/enable", post(enable))
        .route("/plugins/{name}/disable", post(disable))
        .route("/plugins/call", post(call))
}

async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:view").await?;
    let result = state.openuds.command("plugin.list", json!({})).await.map_err(map_err)?;
    Ok(Json(result))
}

async fn info(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:view").await?;
    let result = state.openuds.command("plugin.info", json!({ "name": name })).await.map_err(map_err)?;
    Ok(Json(result))
}

async fn enable(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:manage").await?;
    let result = state.openuds.command("plugin.enable", json!({ "name": name })).await.map_err(map_err)?;
    Ok(Json(result))
}

async fn disable(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:manage").await?;
    let result = state.openuds.command("plugin.disable", json!({ "name": name })).await.map_err(map_err)?;
    Ok(Json(result))
}

/// POST /api/v1/admin/plugins — reload all plugins.
async fn reload(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:manage").await?;
    let result = state.openuds.command("plugin.reload", json!({})).await.map_err(map_err)?;
    Ok(Json(result))
}

async fn remove(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:manage").await?;
    let _ = state.openuds.command("plugin.remove", json!({ "name": name })).await.map_err(map_err)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct PluginCallBody {
    pub name: String,
    pub method: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

async fn call(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PluginCallBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "plugin:call").await?;
    let result = state
        .openuds
        .command("plugin.call", json!({ "name": body.name, "method": body.method, "args": body.args }))
        .await
        .map_err(map_err)?;
    Ok(Json(result))
}

fn map_err(e: OpenUdsError) -> ApiError {
    ApiError::from(e)
}
