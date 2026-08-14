//! `/api/v1/admin/config/*` routes (design §20).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::pmp::{pmp_config_descriptor, pmp_config_groups, ConfigFieldGroup, PmpConfigManager};
use super::repo::{self as config_repo, ConfigSnapshot};
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
use crate::error::extractors::ApiJson;
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
        .route("/config/ppf", get(ppf_config).put(ppf_config_update))
}

// Canonical config surface: descriptors / redacted values / validate / diff / save / snapshots / raw read / rollback.

// ── §17 unified config endpoints ───────────────────────────────

/// GET /api/v1/admin/config/descriptors — Form Descriptors (§22 model A:
/// `{ version, groups: [{ key, label, fields }] }`).
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/descriptors",
    operation_id = "admin_config_descriptors_get",
    responses(
        (status = 200, description = "form descriptors", body = ConfigDescriptorsResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn descriptors(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConfigDescriptorsResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    Ok(Json(ConfigDescriptorsResponse {
        version: 1,
        groups: pmp_config_groups(),
    }))
}

/// GET /api/v1/admin/config/values — current PMP config field values (redacted).
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/values",
    operation_id = "admin_config_values_get",
    responses(
        (status = 200, description = "current config values", body = ConfigValuesResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn values(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConfigValuesResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"));
    }
    let yaml = manager.read_yaml()?;
    let mut values: HashMap<String, Value> = HashMap::new();
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
    Ok(Json(ConfigValuesResponse { version: 1, values }))
}

/// Form-value edit body (§22 model A): Panel submits `{path: value}` and PPB
/// validates/generates YAML/saves.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConfigValuesBody {
    pub values: Value,
    #[serde(default)]
    pub note: String,
}

/// §22 typed descriptors response `{ version, groups }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigDescriptorsResponse {
    pub version: i64,
    pub groups: Vec<ConfigFieldGroup>,
}

/// §22 typed values response `{ version, values }` (flat `{path: value}`).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigValuesResponse {
    pub version: i64,
    pub values: HashMap<String, Value>,
}

/// Stable machine-readable field validation issue. `message` is a debug/legacy
/// fallback only; official Panel UI localizes `code + params` and never renders
/// this string as product copy.
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ConfigValidationIssueCode {
    ValuesMustBeObject,
    ExpectedType,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigValidationError {
    pub path: String,
    pub code: ConfigValidationIssueCode,
    #[serde(default)]
    pub params: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// §22 typed validate response `{ ok, errors }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigValidateResponse {
    pub ok: bool,
    pub errors: Vec<ConfigValidationError>,
}

/// One field-level diff entry `{ path, old, new }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigDiffChange {
    pub path: String,
    pub old: Value,
    pub new: Value,
}

/// §22 typed diff response `{ changes }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigDiffResponse {
    pub changes: Vec<ConfigDiffChange>,
}

/// §22 typed save response `{ ok, snapshot_id }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigSaveResponse {
    pub ok: bool,
    pub snapshot_id: Uuid,
}

/// §22 typed snapshots response `{ items }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigSnapshotsResponse {
    pub items: Vec<ConfigSnapshot>,
}

/// Typed rollback response `{ ok, restored, health }`.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ConfigRollbackResponse {
    pub ok: bool,
    pub restored: Uuid,
    pub health: Value,
}

/// §23 Stop-ship: read the existing YAML, patch ONLY the descriptor fields the
/// user actually modified, keep old values for `[REDACTED]` sentinels, and
/// preserve all unknown (non-descriptor) fields. Never rebuild the whole file.
///
/// Known limitation: `serde_yaml::Value` does not carry YAML comments, so a
/// merge round-trip (parse → patch → serialize) drops any comments present in
/// the source file. Preserving comments would require a lossless YAML editor
/// (e.g. `yaml_edit`/`yaml-rust` style AST); this is recorded as a known
/// limitation, not a silent corruption — non-comment content is preserved.
fn merge_yaml_patch(current_yaml: &str, values: &Value) -> Result<String, ApiError> {
    let mut root: serde_yaml::Value = serde_yaml::from_str(current_yaml)
        .map_err(|e| ApiError::new(ErrorCode::ConfigValidationFailed, format!("existing config is not valid YAML: {e}")))?;
    let obj = values
        .as_object()
        .ok_or_else(|| ApiError::new(ErrorCode::ConfigValidationFailed, "values must be an object"))?;
    for f in pmp_config_descriptor() {
        let Some(v) = obj.get(f.path) else { continue };
        // Sensitive field sentinel (case-insensitive): keep the old value —
        // never write the placeholder.
        if f.sensitive
            && v.as_str()
                .map(|s| s.eq_ignore_ascii_case("[REDACTED]"))
                .unwrap_or(false)
        {
            continue;
        }
        let parts: Vec<&str> = f.path.split('.').collect();
        set_yaml_path(&mut root, &parts, v)?;
    }
    serde_yaml::to_string(&root)
        .map_err(|e| ApiError::new(ErrorCode::ConfigValidationFailed, format!("yaml serialize failed: {e}")))
}

fn set_yaml_path(root: &mut serde_yaml::Value, parts: &[&str], value: &Value) -> Result<(), ApiError> {
    if parts.is_empty() {
        return Ok(());
    }
    let mapping = match root {
        serde_yaml::Value::Mapping(m) => m,
        _ => return Err(ApiError::new(ErrorCode::ConfigValidationFailed, "existing config segment is not a mapping")),
    };
    let key = serde_yaml::Value::String(parts[0].to_string());
    if parts.len() == 1 {
        mapping.insert(key, json_to_yaml(value));
        return Ok(());
    }
    if !mapping.contains_key(&key) {
        mapping.insert(key.clone(), serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    }
    match mapping.get_mut(&key) {
        Some(child) => set_yaml_path(child, &parts[1..], value),
        None => Err(ApiError::new(ErrorCode::ConfigValidationFailed, format!("nested path conflict at {}", parts[0]))),
    }
}

fn json_to_yaml(v: &Value) -> serde_yaml::Value {
    match v {
        Value::Null => serde_yaml::Value::Null,
        Value::Bool(b) => serde_yaml::Value::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yaml::Value::Number(serde_yaml::Number::from(i))
            } else if let Some(f) = n.as_f64() {
                serde_yaml::Value::Number(serde_yaml::Number::from(f))
            } else {
                serde_yaml::Value::Null
            }
        }
        Value::String(s) => serde_yaml::Value::String(s.clone()),
        Value::Array(a) => serde_yaml::Value::Sequence(a.iter().map(json_to_yaml).collect()),
        Value::Object(o) => serde_yaml::Value::Mapping(
            o.iter()
                .map(|(k, v)| (serde_yaml::Value::String(k.clone()), json_to_yaml(v)))
                .collect(),
        ),
    }
}

/// Validate form values against the descriptor; returns `{ ok, errors }`.
fn validate_values(values: &Value) -> (bool, Vec<ConfigValidationError>) {
    let mut errors = Vec::new();
    let obj = match values.as_object() {
        Some(o) => o,
        None => {
            return (
                false,
                vec![ConfigValidationError {
                    path: String::new(),
                    code: ConfigValidationIssueCode::ValuesMustBeObject,
                    params: json!({}),
                    message: Some("values must be an object".to_string()),
                }],
            );
        }
    };
    for f in pmp_config_descriptor() {
        let Some(v) = obj.get(f.path) else { continue };
        if f.sensitive
            && v.as_str()
                .map(|s| s.eq_ignore_ascii_case("[REDACTED]"))
                .unwrap_or(false)
        {
            continue; // unchanged redacted placeholder is fine
        }
        let type_ok = match f.r#type {
            "boolean" => v.is_boolean(),
            "number" => v.is_number(),
            _ => true, // strings / lists / anything
        };
        if !type_ok {
            errors.push(ConfigValidationError {
                path: f.path.to_string(),
                code: ConfigValidationIssueCode::ExpectedType,
                params: json!({ "expected": f.r#type }),
                message: Some(format!("expected {}", f.r#type)),
            });
        }
    }
    (errors.is_empty(), errors)
}

/// POST /api/v1/admin/config/validate — validate form values (§22 `{ ok, errors }`).
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/validate",
    operation_id = "admin_config_validate_post",
    request_body = ConfigValuesBody,
    responses(
        (status = 200, description = "validation result", body = ConfigValidateResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn validate(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<ConfigValuesBody>,
) -> Result<Json<ConfigValidateResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    let (ok, errors) = validate_values(&body.values);
    Ok(Json(ConfigValidateResponse { ok, errors }))
}

/// POST /api/v1/admin/config/diff — field-level diff (§22 `{ changes: [{path, old, new}] }`).
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/diff",
    operation_id = "admin_config_diff_post",
    request_body = ConfigValuesBody,
    responses(
        (status = 200, description = "field-level diff", body = ConfigDiffResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn diff(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<ConfigValuesBody>,
) -> Result<Json<ConfigDiffResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    let current = manager.read_yaml()?;
    // §23: diff against the actual merge result (redacted kept, unknowns preserved).
    let proposed = merge_yaml_patch(&current, &body.values)?;
    let mut changes: Vec<ConfigDiffChange> = Vec::new();
    for f in pmp_config_descriptor() {
        let a = manager
            .field_value(&current, f.path)
            .map(|y| serde_yaml::from_value::<Value>(y).unwrap_or(Value::Null));
        let b = manager
            .field_value(&proposed, f.path)
            .map(|y| serde_yaml::from_value::<Value>(y).unwrap_or(Value::Null));
        if a != b {
            changes.push(ConfigDiffChange {
                path: f.path.to_string(),
                old: if f.sensitive { Value::String("[REDACTED]".into()) } else { a.unwrap_or(Value::Null) },
                new: if f.sensitive { Value::String("[REDACTED]".into()) } else { b.unwrap_or(Value::Null) },
            });
        }
    }
    Ok(Json(ConfigDiffResponse { changes }))
}

/// POST /api/v1/admin/config/save — validate → snapshot → generate YAML →
/// write → reload (§22 `{ ok, snapshot_id }`).
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/save",
    operation_id = "admin_config_save_post",
    request_body = ConfigValuesBody,
    responses(
        (status = 200, description = "saved + snapshot id", body = ConfigSaveResponse),
        (status = 401, description = "critical reauth required", body = ErrorEnvelope),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn save(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<ConfigValuesBody>,
) -> Result<Json<ConfigSaveResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    crate::auth::routes::check_reauth_header(
        &state,
        &auth,
        &headers,
        crate::auth::reauth::ReauthRisk::Critical,
    )?;
    let (ok, errors) = validate_values(&body.values);
    if !ok {
        return Err(ApiError::new(ErrorCode::ConfigValidationFailed, "invalid config values"));
    }
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"));
    }
    // §23: snapshot the ORIGINAL config, then merge/patch (read → patch only
    // modified descriptor fields → keep [REDACTED] old values → preserve
    // unknown fields → validate → atomic write). Never rebuild from values.
    let prev = manager.read_yaml()?;
    let yaml = merge_yaml_patch(&prev, &body.values)?;
    // validate the merged YAML still parses.
    serde_yaml::from_str::<serde_yaml::Value>(&yaml)
        .map_err(|e| ApiError::new(ErrorCode::ConfigValidationFailed, format!("merged YAML invalid: {e}")))?;
    let db = state.require_db()?;
    let snapshot_id = config_repo::insert_snapshot(db, "pmp", &prev, &format!("pre-save: {}", body.note), Some(auth.sub)).await?;
    manager.write_yaml_atomic(&yaml)?;
    state
        .openuds
        .command("server.config_reload", serde_json::json!({}))
        .await
        .map_err(ApiError::from)?;

    let changed_paths = body
        .values
        .as_object()
        .map(|values| values.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    if let Err(error) = crate::audit::service::record_principal(
        db,
        &auth,
        "config.save",
        "config",
        "pmp",
        json!({
            "changed_paths": changed_paths,
            "snapshot_id": snapshot_id,
        }),
        "succeeded",
        "",
        "",
        &ip_from_headers(&headers),
        &user_agent_from_headers(&headers),
    )
    .await
    {
        tracing::error!(%error, snapshot_id = %snapshot_id, "config save audit record failed");
    }

    Ok(Json(ConfigSaveResponse { ok: true, snapshot_id }))
}

/// GET /api/v1/admin/config/snapshots — list PMP snapshots (§22 `{ items }`).
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/snapshots",
    operation_id = "admin_config_snapshots_get",
    responses(
        (status = 200, description = "snapshot list", body = ConfigSnapshotsResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn snapshots(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<ConfigSnapshotsResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let db = state.require_db()?;
    let snaps = config_repo::list_snapshots(db, "pmp", 200).await?;
    Ok(Json(ConfigSnapshotsResponse { items: snaps }))
}

/// GET /api/v1/admin/config/raw — redacted read-only YAML projection.
///
/// This is deliberately NOT the literal on-disk file: only descriptor-owned
/// fields are projected, sensitive fields are replaced with `[REDACTED]`, and
/// unknown fields are omitted. The save path still merges against the real
/// source YAML server-side, so unknown fields remain preserved without ever
/// being echoed to the browser.
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/raw",
    operation_id = "admin_config_raw_get",
    responses(
        (status = 200, description = "redacted canonical config YAML projection", body = String, content_type = "text/plain"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn raw(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<String, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let manager = PmpConfigManager::new(state.config.pmp.config_path.clone());
    if !manager.configured() {
        return Err(ApiError::new(ErrorCode::PmpUnavailable, "pmp.config_path not configured"));
    }

    let source = manager.read_yaml()?;
    let mut projected = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    for field in pmp_config_descriptor() {
        let Some(raw_value) = manager.field_value(&source, field.path) else {
            continue;
        };
        let value = if field.sensitive {
            Value::String("[REDACTED]".to_string())
        } else {
            serde_yaml::from_value::<Value>(raw_value).unwrap_or(Value::Null)
        };
        let parts: Vec<&str> = field.path.split('.').collect();
        set_yaml_path(&mut projected, &parts, &value)?;
    }
    let yaml = serde_yaml::to_string(&projected)
        .map_err(|e| ApiError::new(ErrorCode::ConfigValidationFailed, format!("yaml projection failed: {e}")))?;
    Ok(format!(
        "# Redacted canonical configuration projection; unknown fields are omitted.\n{yaml}"
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RollbackBody {
    pub snapshot_id: Uuid,
}

/// POST /api/v1/admin/config/rollback — rollback to a snapshot.
#[utoipa::path(
    post,
    path = "/api/v1/admin/config/rollback",
    operation_id = "admin_config_rollback_post",
    request_body = RollbackBody,
    responses(
        (status = 200, description = "rolled back + health", body = ConfigRollbackResponse),
        (status = 401, description = "critical reauth required", body = ErrorEnvelope),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn rollback(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<RollbackBody>,
) -> Result<Json<ConfigRollbackResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:rollback").await?;
    // §23 #10: config rollback requires reauth.
    crate::auth::routes::check_reauth_header(&state, &auth, &headers, crate::auth::reauth::ReauthRisk::Critical)?;
    let db = state.require_db()?;
    let snapshot = config_repo::get_snapshot(db, body.snapshot_id)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::ConfigSnapshotNotFound, "config snapshot not found"))?;
    if snapshot.scope != "pmp" {
        return Err(ApiError::new(ErrorCode::ConfigScopeInvalid, "snapshot scope must be pmp"));
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

    if let Err(error) = crate::audit::service::record_principal(
        db,
        &auth,
        "config.rollback",
        "config",
        "pmp",
        json!({ "snapshot_id": body.snapshot_id }),
        "succeeded",
        "",
        "",
        &ip_from_headers(&headers),
        &user_agent_from_headers(&headers),
    )
    .await
    {
        tracing::error!(%error, snapshot_id = %body.snapshot_id, "config rollback audit record failed");
    }

    Ok(Json(ConfigRollbackResponse {
        ok: true,
        restored: body.snapshot_id,
        health,
    }))
}

// ── PPF build/SEO config ────────────────────────────────────────

/// GET /api/v1/admin/config/ppf — PPF build/SEO config.
#[utoipa::path(
    get,
    path = "/api/v1/admin/config/ppf",
    operation_id = "admin_config_ppf_get",
    responses(
        (status = 200, description = "PPF build config", body = PpfBuildConfigResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn ppf_config(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<PpfBuildConfigResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:view").await?;
    let db = state.require_db()?;
    match config_repo::get_ppf_config(db).await? {
        Some((revision, content)) => Ok(Json(PpfBuildConfigResponse { revision, content })),
        None => Ok(Json(PpfBuildConfigResponse { revision: 0, content: json!({}) })),
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PpConfigBody2 {
    pub content: Value,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct PpfBuildConfigResponse {
    pub revision: i64,
    pub content: Value,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct PpfBuildConfigSaveResponse {
    pub ok: bool,
    pub revision: i64,
}

#[utoipa::path(
    put,
    path = "/api/v1/admin/config/ppf",
    operation_id = "admin_config_ppf_put",
    request_body = PpConfigBody2,
    responses(
        (status = 200, description = "PPF build config saved", body = PpfBuildConfigSaveResponse),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
        (status = 422, description = "invalid build config", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn ppf_config_update(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<PpConfigBody2>,
) -> Result<Json<PpfBuildConfigSaveResponse>, ApiError> {
    state.permissions.require(&state.db, &auth, "config:reload").await?;
    validate_ppf_build_config(&body.content)?;
    let db = state.require_db()?;
    let revision = config_repo::put_ppf_config(db, body.content, Some(auth.sub)).await?;
    Ok(Json(PpfBuildConfigSaveResponse { ok: true, revision }))
}

fn validate_ppf_build_config(content: &Value) -> Result<(), ApiError> {
    let object = content.as_object().ok_or_else(|| ApiError::new(ErrorCode::ConfigValidationFailed, "PPF build config must be an object"))?;
    for key in object.keys() {
        if !matches!(key.as_str(), "site_name" | "site_description" | "canonical_url" | "analytics_provider" | "plausible_domain" | "ga_id" | "search_verification_google" | "search_verification_bing") {
            return Err(ApiError::new(ErrorCode::ConfigValidationFailed, "unsupported PPF build config key"));
        }
    }
    if let Some(url) = object.get("canonical_url").and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()) {
        let parsed = reqwest::Url::parse(url).map_err(|_| ApiError::new(ErrorCode::ConfigValidationFailed, "canonical_url must be an absolute URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ApiError::new(ErrorCode::ConfigValidationFailed, "canonical_url must use http or https"));
        }
    }
    if let Some(provider) = object.get("analytics_provider").and_then(Value::as_str) {
        if !matches!(provider, "" | "plausible" | "ga4") {
            return Err(ApiError::new(ErrorCode::ConfigValidationFailed, "analytics_provider must be plausible or ga4"));
        }
    }
    Ok(())
}

fn ip_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string()
}

fn user_agent_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_unknown_fields_and_redacted() {
        let current = r#"server_name: "Old Server"
welcome: "hi"
custom_unknown_key: "keep-me"
openuds:
  enabled: true
  socket_path: "/run/old.sock"
  auth_token: "SECRET"
"#;
        let values = serde_json::json!({
            "server_name": "New Server",
            "openuds.auth_token": "[REDACTED]",
        });
        let merged = merge_yaml_patch(current, &values).unwrap();
        // patched field
        assert!(merged.contains("server_name: New Server"), "merged: {merged}");
        // unknown field preserved
        assert!(merged.contains("custom_unknown_key: keep-me"), "merged: {merged}");
        // redacted sensitive field keeps old value, never writes placeholder
        assert!(merged.contains("auth_token: SECRET"), "merged: {merged}");
        assert!(!merged.contains("REDACTED"), "merged: {merged}");
        // untouched field preserved
        assert!(merged.contains("socket_path: /run/old.sock"), "merged: {merged}");
    }

    #[test]
    fn merge_redacted_case_insensitive() {
        let current = "database_url: \"postgres://secret\"\n";
        for sentinel in ["[REDACTED]", "[redacted]", "[Redacted]"] {
            let values = serde_json::json!({ "database_url": sentinel });
            let merged = merge_yaml_patch(current, &values).unwrap();
            assert!(merged.contains("database_url: postgres://secret"), "merged: {merged}");
        }
    }

    #[test]
    fn merge_patches_nested_descriptor_path() {
        let current = "openuds:\n  enabled: false\n  socket_path: \"/x\"\n";
        let values = serde_json::json!({ "openuds.enabled": true });
        let merged = merge_yaml_patch(current, &values).unwrap();
        assert!(merged.contains("enabled: true"), "merged: {merged}");
        assert!(merged.contains("socket_path: /x"), "merged: {merged}");
    }
}
