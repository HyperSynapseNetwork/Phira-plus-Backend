//! `/api/v1/admin/audit/*` routes (Panel §18.12).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;

use super::model::AuditEvent;
use super::repo;
use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/audit", get(list))
        .route("/audit/{id}", get(detail))
        .route("/audit/export", get(export))
        .route("/audit/export.csv", get(export_csv))
}

#[derive(Debug, Deserialize)]
pub struct AuditFilterParams {
    pub action: Option<String>,
    pub principal_type: Option<String>,
    pub actor: Option<Uuid>,
    pub result: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

/// GET /api/v1/admin/audit — filtered audit list.
#[utoipa::path(
    get,
    path = "/api/v1/admin/audit",
    operation_id = "admin_audit_get",
    responses(
        (status = 200, description = "audit events (paginated)", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditFilterParams>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "audit:view").await?;
    let db = state.require_db()?;
    let page = params.page.unwrap_or(1).max(1);
    let page_num = params.page_num.unwrap_or(20);
    if !(1..=100).contains(&page_num) {
        return Err(ApiError::validation("pageNum must be between 1 and 100"));
    }
    let offset = (page - 1) * page_num;

    let events = repo::list_filtered(
        db,
        params.action.as_deref(),
        params.principal_type.as_deref(),
        params.actor,
        params.result.as_deref(),
        page_num,
        offset,
    )
    .await?;

    Ok(Json(serde_json::json!({
        "items": events,
        "total": events.len() as i64,
        "page": page,
        "pageNum": page_num,
    })))
}

/// GET /api/v1/admin/audit/{id} — single audit event.
#[utoipa::path(
    get,
    path = "/api/v1/admin/audit/{id}",
    operation_id = "admin_audit_id_get",
    responses(
        (status = 200, description = "audit event", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn detail(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AuditEvent>, ApiError> {
    state.permissions.require(&state.db, &auth, "audit:view").await?;
    let db = state.require_db()?;
    let event = repo::get(db, id).await?.ok_or_else(|| ApiError::not_found("audit event"))?;
    Ok(Json(event))
}

#[derive(Debug, Deserialize)]
pub struct ExportParams {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(flatten)]
    pub filter: AuditFilterParams,
}

/// GET /api/v1/admin/audit/export?format=csv|json — export (contract §17).
#[utoipa::path(
    get,
    path = "/api/v1/admin/audit/export",
    operation_id = "admin_audit_export_get",
    responses(
        (status = 200, description = "audit export (csv|json)", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn export(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ExportParams>,
) -> Result<axum::response::Response, ApiError> {
    let db = state.require_db()?;
    let events = repo::list_filtered(
        db,
        params.filter.action.as_deref(),
        params.filter.principal_type.as_deref(),
        params.filter.actor,
        params.filter.result.as_deref(),
        10_000,
        0,
    )
    .await?;

    match params.format.as_deref() {
        Some("csv") => {
            state.permissions.require(&state.db, &auth, "audit:export").await?;
            Ok(csv_response(events))
        }
        Some("json") | None => {
            state.permissions.require(&state.db, &auth, "audit:view").await?;
            Ok(Json(serde_json::json!({ "items": events })).into_response())
        }
        Some(other) => Err(ApiError::validation(format!("format must be csv or json, got {other}"))),
    }
}

/// GET /api/v1/admin/audit/export.csv — CSV export (redacted; no secrets).
#[utoipa::path(
    get,
    path = "/api/v1/admin/audit/export.csv",
    operation_id = "admin_audit_export_csv_get",
    responses(
        (status = 200, description = "audit CSV export", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn export_csv(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditFilterParams>,
) -> Result<axum::response::Response, ApiError> {
    state.permissions.require(&state.db, &auth, "audit:export").await?;
    let db = state.require_db()?;
    let events = repo::list_filtered(
        db,
        params.action.as_deref(),
        params.principal_type.as_deref(),
        params.actor,
        params.result.as_deref(),
        10_000,
        0,
    )
    .await?;
    Ok(csv_response(events))
}

fn csv_response(events: Vec<AuditEvent>) -> axum::response::Response {
    let mut csv = String::from(
        "id,occurred_at,principal_type,actor_user_id,action,resource_type,resource_id,result,error_code,request_id,command_id,ip\n",
    );
    for e in &events {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}\n",
            e.id,
            e.occurred_at,
            e.principal_type,
            e.actor_user_id.map(|u| u.to_string()).unwrap_or_default(),
            csv_escape(&e.action),
            csv_escape(&e.resource_type),
            csv_escape(&e.resource_id),
            e.result,
            csv_escape(&e.error_code),
            csv_escape(&e.request_id),
            csv_escape(&e.command_id),
            csv_escape(&e.ip),
        ));
    }

    let mut resp = (StatusCode::OK, csv).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=audit.csv"),
    );
    resp
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escape_plain() {
        assert_eq!(csv_escape("room.kick"), "room.kick");
    }

    #[test]
    fn csv_escape_quotes_comma_and_newline() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }
}
