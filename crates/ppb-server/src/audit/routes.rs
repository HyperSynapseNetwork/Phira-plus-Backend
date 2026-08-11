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
use crate::error::ApiError;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/audit", get(list))
        .route("/audit/{id}", get(detail))
        .route("/audit/export.csv", get(export_csv))
}

#[derive(Debug, Deserialize)]
pub struct AuditFilterParams {
    pub action: Option<String>,
    #[serde(rename = "principalType")]
    pub principal_type: Option<String>,
    pub actor: Option<Uuid>,
    pub result: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageNum")]
    pub page_num: Option<i64>,
}

async fn list(
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

async fn detail(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AuditEvent>, ApiError> {
    state.permissions.require(&state.db, &auth, "audit:view").await?;
    let db = state.require_db()?;
    let event = repo::get(db, id).await?.ok_or_else(|| ApiError::not_found("audit event"))?;
    Ok(Json(event))
}

/// GET /api/v1/admin/audit/export.csv — CSV export (redacted; no secrets).
async fn export_csv(
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
    Ok(resp)
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
