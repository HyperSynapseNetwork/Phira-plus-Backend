//! `/api/v1/me/preferences/{namespace}` — namespaced preferences (contract §7, §20).
//!
//! Wires the existing preferences domain (preferences/mod.rs) to the HTTP layer:
//! GET/PUT/DELETE by namespace with revision optimistic concurrency.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/me/preferences/{namespace}", get(get_one).put(update).delete(delete_one))
}

fn validate_namespace(namespace: &str) -> Result<(), ApiError> {
    if matches!(namespace, "common" | "ppf" | "panel" | "experiments") {
        Ok(())
    } else {
        Err(ApiError::validation("namespace must be common|ppf|panel|experiments"))
    }
}

/// GET /api/v1/me/preferences/{namespace} — one namespaced preference.
#[utoipa::path(
    get,
    path = "/api/v1/me/preferences/{namespace}",
    operation_id = "me_preferences_namespace_get",
    responses(
        (status = 200, description = "namespaced preferences", body = serde_json::Value),
        (status = 404, description = "namespace not set", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn get_one(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    validate_namespace(&namespace)?;
    let db = state.require_db()?;
    match crate::preferences::get(db, auth.sub, &namespace).await? {
        Some(p) => Ok(Json(serde_json::to_value(p).unwrap_or(Value::Null))),
        None => Err(ApiError::not_found("preferences")),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdatePreferencesBody {
    pub data: Value,
    #[serde(default)]
    pub base_revision: Option<i64>,
}

/// PUT /api/v1/me/preferences/{namespace} — upsert with optimistic concurrency.
#[utoipa::path(
    put,
    path = "/api/v1/me/preferences/{namespace}",
    operation_id = "me_preferences_namespace_put",
    request_body = UpdatePreferencesBody,
    responses(
        (status = 200, description = "saved preferences", body = serde_json::Value),
        (status = 409, description = "revision mismatch", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
    Json(body): Json<UpdatePreferencesBody>,
) -> Result<Json<Value>, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    validate_namespace(&namespace)?;
    let db = state.require_db()?;
    let row = crate::preferences::upsert(db, auth.sub, &namespace, body.data, body.base_revision).await?;
    Ok(Json(serde_json::to_value(row).unwrap_or(Value::Null)))
}

/// DELETE /api/v1/me/preferences/{namespace} — delete a namespaced preference.
#[utoipa::path(
    delete,
    path = "/api/v1/me/preferences/{namespace}",
    operation_id = "me_preferences_namespace_delete",
    responses(
        (status = 204, description = "deleted"),
        (status = 401, description = "unauthenticated", body = ErrorEnvelope),
    ),
    tag = "me"
)]
pub async fn delete_one(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(namespace): Path<String>,
) -> Result<StatusCode, ApiError> {
    if auth.is_root() {
        return Err(ApiError::permission_denied());
    }
    validate_namespace(&namespace)?;
    let db = state.require_db()?;
    crate::preferences::delete(db, auth.sub, &namespace).await?;
    Ok(StatusCode::NO_CONTENT)
}
