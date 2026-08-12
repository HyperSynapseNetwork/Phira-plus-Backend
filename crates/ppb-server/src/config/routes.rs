//! `/api/v1/admin/config/*` routes (design §20).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::pmp::{pmp_config_descriptor, PmpConfigManager};
use super::repo as config_repo;
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/config/descriptors", get(descriptors))
        .route("/config/values", get(values))
        .route("/config/validate", post(validate))
        .route("/config/diff", post(diff))
        .route("/config/save", post(save))
        .route("/config/snapshots", get(snapshots))
        .route("/config/raw", get(raw))
        .route("/config/rollback", post(rollback))
        .route("/config/ppb", get(ppb_config).put(ppb_config_update))
        .route("/config/pmp/descriptor", get(pmp_descriptor))
        .route("/config/pmp", get(pmp_config).put(pmp_config_update))
        .route("/config/pmp/snapshots", get(pmp_snapshots).post(pmp_snapshot_create))
        .route("/config/pmp/snapshots/{id}/rollback", post(pmp_snapshot_rollback))
        .route("/config/ppf", get(ppf_config).put(ppf_config_update))
        .route("/config/public/{key}", get(public_content).put(public_content_update))
}

// ── PPB runtime config ──────────────────────────────────────────

/// GET /api/v1/admin/config/ppb — effective PPB config (merged with overrides).
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/ppb",
    responses(
        (status = 200, description = "effective PPB config", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn ppb_config(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let effective = effective_config(&state).await?;
    Ok(Json(effective))
}

#[derive(Debug, Deserialize)]
pub struct PpConfigBody {
    pub overrides: Value,
}

/// PUT /api/v1/admin/config/ppb — validate + store runtime overrides.
async fn ppb_config_update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PpConfigBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    let db = state.require_db()?;
    // Validate by applying to a copy.
    state
        .config
        .apply_overrides(&body.overrides)
        .map_err(|e| ApiError::validation(format!("invalid overrides: {e}")))?;
    config_repo::put_overrides(db, body.overrides.clone(), Some(auth.sub)).await?;
    Ok(Json(json!({ "ok": true, "overrides": body.overrides })))
}

async fn effective_config(state: &Arc<AppState>) -> Result<Value, ApiError> {
    let base = serde_json::to_value(&*state.config)
        .map_err(|e| ApiError::new(crate::error::ErrorCode::Internal, e.to_string()))?;
    if let Some(db) = &state.db {
        if let Some(over) = config_repo::get_overrides(db).await? {
            let mut merged = base;
            if let (Some(b), Some(o)) = (merged.as_object_mut(), over.as_object()) {
                for (k, v) in o {
                    if b.contains_key(k) {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
            return Ok(merged);
        }
    }
    Ok(base)
}

// ── PMP config ──────────────────────────────────────────────────

async fn pmp_descriptor(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    Ok(Json(serde_json::json!({
        "version": 1,
        "fields": pmp_config_descriptor(),
    })))
}

// ── §17 unified config endpoints ───────────────────────────────

/// GET /api/v1/admin/config/descriptors — Form Descriptors for all scopes.
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/descriptors",
    responses(
        (status = 200, description = "form descriptors", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn descriptors(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    Ok(Json(serde_json::json!({
        "version": 1,
        "pmp": pmp_config_descriptor(),
        "ppb": [], // JSON-scoped; Panel edits via /config/ppb
        "ppf": [], // JSON-scoped; Panel edits via /config/ppf
    })))
}

/// GET /api/v1/admin/config/values — current PMP config field values (redacted).
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/values",
    responses(
        (status = 200, description = "current config values", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn values(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"));
    }
    let yaml = manager.read_yaml()?;
    let mut values = serde_json::Map::new();
    for f in pmp_config_descriptor() {
        let v = manager.field_value(&yaml, f.path);
        let value = if f.sensitive {
            if v.is_some() {
                Value::String("[REDACTED]".to_string())
            } else {
                Value::Null
            }
        } else {
            v.map(|y| serde_yaml::from_value::<Value>(y).unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        };
        values.insert(f.path.to_string(), value);
    }
    Ok(Json(serde_json::json!({ "version": 1, "values": values })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConfigContentBody {
    pub content: String,
    #[serde(default)]
    pub note: String,
}

/// POST /api/v1/admin/config/validate — validate proposed YAML parses.
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/validate",
    request_body = ConfigContentBody,
    responses(
        (status = 200, description = "validation result", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn validate(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConfigContentBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    serde_yaml::from_str::<serde_yaml::Value>(&body.content)
        .map_err(|e| ApiError::validation(format!("invalid YAML: {e}")))?;
    Ok(Json(serde_json::json!({ "valid": true })))
}

/// POST /api/v1/admin/config/diff — field-level diff of current vs proposed.
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/diff",
    request_body = ConfigContentBody,
    responses(
        (status = 200, description = "field-level diff", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn diff(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConfigContentBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    let current = manager.read_yaml().unwrap_or_default();
    let _ = serde_yaml::from_str::<serde_yaml::Value>(&body.content)
        .map_err(|e| ApiError::validation(format!("invalid YAML: {e}")))?;
    let mut changes = Vec::new();
    for f in pmp_config_descriptor() {
        let a = manager
            .field_value(&current, f.path)
            .map(|y| serde_yaml::to_string(&y).unwrap_or_default());
        let b = manager
            .field_value(&body.content, f.path)
            .map(|y| serde_yaml::to_string(&y).unwrap_or_default());
        if a != b {
            changes.push(serde_json::json!({
                "path": f.path,
                "before": if f.sensitive { "[REDACTED]".to_string() } else { a.unwrap_or_default() },
                "after": if f.sensitive { "[REDACTED]".to_string() } else { b.unwrap_or_default() },
            }));
        }
    }
    Ok(Json(serde_json::json!({ "changes": changes, "changed": !changes.is_empty() })))
}

/// POST /api/v1/admin/config/save — validate → snapshot → atomic write →
/// reload → health check (design §20.3).
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/save",
    request_body = ConfigContentBody,
    responses(
        (status = 200, description = "saved + health", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn save(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ConfigContentBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    serde_yaml::from_str::<serde_yaml::Value>(&body.content)
        .map_err(|e| ApiError::validation(format!("invalid YAML: {e}")))?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"));
    }
    let db = state.require_db()?;
    let prev = manager.read_yaml().ok();
    if let Some(prev) = prev {
        config_repo::insert_snapshot(db, "pmp", &prev, &format!("pre-save: {}", body.note), Some(auth.sub)).await?;
    }
    manager.write_yaml_atomic(&body.content)?;
    state
        .openuds
        .command("server.config_reload", serde_json::json!({}))
        .await
        .map_err(ApiError::from)?;
    let health = health_check(&state).await;
    Ok(Json(serde_json::json!({ "ok": true, "note": body.note, "health": health })))
}

/// GET /api/v1/admin/config/snapshots — list PMP snapshots.
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/snapshots",
    responses(
        (status = 200, description = "snapshot list", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn snapshots(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let db = state.require_db()?;
    let snaps = config_repo::list_snapshots(db, "pmp", 200).await?;
    Ok(Json(serde_json::json!({ "snapshots": snaps })))
}

/// GET /api/v1/admin/config/raw — raw PMP config YAML.
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/raw",
    responses(
        (status = 200, description = "raw config YAML", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn raw(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"));
    }
    Ok(Json(serde_json::json!({ "content": manager.read_yaml()? })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RollbackBody {
    pub snapshot_id: Uuid,
}

/// POST /api/v1/admin/config/rollback — rollback to a snapshot.
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/rollback",
    request_body = RollbackBody,
    responses(
        (status = 200, description = "rolled back + health", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn rollback(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RollbackBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:rollback").await?;
    let db = state.require_db()?;
    let snapshot = config_repo::get_snapshot(db, body.snapshot_id)
        .await?
        .ok_or_else(|| ApiError::not_found("snapshot"))?;
    if snapshot.scope != "pmp" {
        return Err(ApiError::validation("snapshot scope must be pmp"));
    }
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    manager.write_yaml_atomic(&snapshot.content)?;
    state
        .openuds
        .command("server.config_reload", serde_json::json!({}))
        .await
        .map_err(ApiError::from)?;
    config_repo::mark_restored(db, body.snapshot_id).await?;
    let health = health_check(&state).await;
    Ok(Json(serde_json::json!({ "ok": true, "restored": body.snapshot_id, "health": health })))
}

async fn pmp_config(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(
            crate::error::ErrorCode::PmpUnavailable,
            "pmp.config_path not configured",
        ));
    }
    let yaml = manager.read_yaml()?;
    let mut values = serde_json::Map::new();
    for f in pmp_config_descriptor() {
        let v = manager.field_value(&yaml, f.path);
        let value = if f.sensitive {
            if v.is_some() {
                Value::String("[REDACTED]".to_string())
            } else {
                Value::Null
            }
        } else {
            v.map(|y| serde_yaml::from_value::<Value>(y).unwrap_or(Value::Null))
                .unwrap_or(Value::Null)
        };
        values.insert(f.path.to_string(), value);
    }
    Ok(Json(json!({ "version": 1, "values": values })))
}

#[derive(Debug, Deserialize)]
pub struct PmpConfigUpdateBody {
    /// Full YAML content (field edits are applied to the whole file).
    pub content: String,
    #[serde(default)]
    pub note: String,
}

async fn pmp_config_update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PmpConfigUpdateBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    // Validate YAML parses before writing.
    serde_yaml::from_str::<serde_yaml::Value>(&body.content)
        .map_err(|e| ApiError::validation(format!("invalid YAML: {e}")))?;
    // Snapshot the pre-change state first.
    let db = state.require_db()?;
    let prev = manager.read_yaml().ok();
    if let Some(prev) = prev {
        config_repo::insert_snapshot(db, "pmp", &prev, &format!("pre-edit: {}", body.note), Some(auth.sub)).await?;
    }
    manager.write_yaml_atomic(&body.content)?;
    // Reload PMP.
    state
        .openuds
        .command("server.config_reload", json!({}))
        .await
        .map_err(crate::error::ApiError::from)?;
    Ok(Json(json!({ "ok": true, "note": body.note })))
}

async fn pmp_snapshots(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let db = state.require_db()?;
    let snapshots = config_repo::list_snapshots(db, "pmp", 200).await?;
    Ok(Json(json!({ "snapshots": snapshots })))
}

async fn pmp_snapshot_create(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PmpConfigUpdateBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    let db = state.require_db()?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    let yaml = manager.read_yaml()?;
    let id = config_repo::insert_snapshot(db, "pmp", &yaml, &body.note, Some(auth.sub)).await?;
    Ok(Json(json!({ "snapshot_id": id })))
}

async fn pmp_snapshot_rollback(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:rollback").await?;
    let db = state.require_db()?;
    let snapshot = config_repo::get_snapshot(db, snapshot_id)
        .await?
        .ok_or_else(|| ApiError::not_found("snapshot"))?;
    if snapshot.scope != "pmp" {
        return Err(ApiError::validation("snapshot scope must be pmp"));
    }
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    manager.write_yaml_atomic(&snapshot.content)?;
    state
        .openuds
        .command("server.config_reload", json!({}))
        .await
        .map_err(crate::error::ApiError::from)?;
    config_repo::mark_restored(db, snapshot_id).await?;
    let health = health_check(&state).await;
    Ok(Json(json!({ "ok": true, "restored": snapshot_id, "health": health })))
}

async fn health_check(state: &Arc<AppState>) -> Value {
    let Some(url) = state.config.pmp.http_url.clone() else {
        return json!({ "checked": false, "note": "pmp.http_url not configured" });
    };
    let result = reqwest::Client::new()
        .get(format!("{url}/health/ready"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await;
    match result {
        Ok(resp) => json!({ "checked": true, "status": resp.status().as_u16() }),
        Err(e) => json!({ "checked": true, "error": e.to_string() }),
    }
}

// ── PPF build/SEO config ────────────────────────────────────────

/// GET /api/v1/admin/config/ppf — PPF build/SEO config.
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/ppf",
    responses(
        (status = 200, description = "PPF build config", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn ppf_config(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let db = state.require_db()?;
    match config_repo::get_ppf_config(db).await? {
        Some((revision, content)) => Ok(Json(json!({ "revision": revision, "content": content }))),
        None => Ok(Json(json!({ "revision": 0, "content": {} }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct PpConfigBody2 {
    pub content: Value,
}

async fn ppf_config_update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PpConfigBody2>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    let db = state.require_db()?;
    let revision = config_repo::put_ppf_config(db, body.content.clone(), Some(auth.sub)).await?;
    Ok(Json(json!({ "ok": true, "revision": revision })))
}

// ── Public runtime content ──────────────────────────────────────

async fn public_content(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    if !matches!(key.as_str(), "site" | "announcements" | "downloads" | "nodes") {
        return Err(ApiError::validation("key must be site|announcements|downloads|nodes"));
    }
    let db = state.require_db()?;
    let content = config_repo::get_public_content(db, &key).await?.unwrap_or(serde_json::Value::Object(Default::default()));
    Ok(Json(json!({ "key": key, "content": content })))
}

async fn public_content_update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(body): Json<PpConfigBody2>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    if !matches!(key.as_str(), "site" | "announcements" | "downloads" | "nodes") {
        return Err(ApiError::validation("key must be site|announcements|downloads|nodes"));
    }
    let db = state.require_db()?;
    config_repo::put_public_content(db, &key, body.content.clone(), Some(auth.sub)).await?;
    Ok(Json(json!({ "ok": true, "key": key })))
}
