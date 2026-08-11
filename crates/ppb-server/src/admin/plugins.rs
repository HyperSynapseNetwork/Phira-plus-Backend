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
        .route("/plugins/{name}/{action}", post(action_dispatch))
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

#[derive(Debug, Deserialize, Default)]
pub struct PluginCallBody {
    pub name: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

/// POST /api/v1/admin/plugins/{name}/{action} — unified plugin action dispatch
/// (contract §17): enable | disable | reload | remove | call.
async fn action_dispatch(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path((name, action)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let parsed: PluginCallBody = if body.is_empty() {
        PluginCallBody::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| ApiError::validation(format!("invalid body: {e}")))?
    };
    match action.as_str() {
        "enable" => {
            state.permissions.require(&state.db, &auth, "plugin:manage").await?;
            Ok(Json(state.openuds.command("plugin.enable", json!({ "name": name })).await.map_err(map_err)?))
        }
        "disable" => {
            state.permissions.require(&state.db, &auth, "plugin:manage").await?;
            Ok(Json(state.openuds.command("plugin.disable", json!({ "name": name })).await.map_err(map_err)?))
        }
        "reload" => {
            state.permissions.require(&state.db, &auth, "plugin:manage").await?;
            Ok(Json(state.openuds.command("plugin.reload", json!({ "name": name })).await.map_err(map_err)?))
        }
        "remove" => {
            state.permissions.require(&state.db, &auth, "plugin:manage").await?;
            Ok(Json(state.openuds.command("plugin.remove", json!({ "name": name })).await.map_err(map_err)?))
        }
        "call" => {
            state.permissions.require(&state.db, &auth, "plugin:call").await?;
            if parsed.method.is_empty() {
                return Err(ApiError::validation("method required for plugin.call"));
            }
            Ok(Json(state.openuds.command("plugin.call", json!({ "name": name, "method": parsed.method, "args": parsed.args })).await.map_err(map_err)?))
        }
        other => Err(ApiError::validation(format!("unknown plugin action: {other}"))),
    }
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
