//! `/api/v1/admin/coupons/*` (design §18.14, contract §17).
//!
//! V1 covers admin create + revoke. Redemption (which executes an Action) is a
//! later phase; the schema already tracks `action_type` / `payload` / `max_uses`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::types::AuthPrincipal;
#[allow(unused_imports)]
use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/coupons", get(list))
        .route("/coupons/create", post(create))
        .route("/coupons/{id}/revoke", post(revoke))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateCouponBody {
    #[serde(default)]
    pub code: String,
    pub action_type: String,
    #[serde(default)]
    pub payload: Value,
    pub max_uses: Option<i32>,
}

/// POST /api/v1/admin/coupons/create — create a coupon (generates a code if blank).
#[utoipa::path(
    post,
    path = "/api/v1/admin/coupons/create",
    request_body = CreateCouponBody,
    responses(
        (status = 200, description = "coupon created", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn create(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateCouponBody>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "coupon:create").await?;
    let db = state.require_db()?;
    let code = if body.code.trim().is_empty() {
        generate_code()
    } else {
        body.code.trim().to_string()
    };
    let payload = if body.payload.is_null() {
        json!({})
    } else {
        body.payload
    };
    let row = sqlx::query_as::<_, (Uuid, String)>(
        "INSERT INTO coupons (code, action_type, payload, max_uses, created_by)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, code",
    )
    .bind(&code)
    .bind(&body.action_type)
    .bind(payload)
    .bind(body.max_uses.unwrap_or(1))
    .bind(auth.sub)
    .fetch_one(db)
    .await
    .map_err(|e| {
        if let sqlx::Error::Database(dbe) = &e {
            if dbe.is_unique_violation() {
                return ApiError::new(ErrorCode::Conflict, "coupon code already exists");
            }
        }
        db_err(e)
    })?;
    Ok(Json(json!({ "id": row.0, "code": row.1 })))
}

/// POST /api/v1/admin/coupons/{id}/revoke — revoke a coupon.
#[utoipa::path(
    post,
    path = "/api/v1/admin/coupons/{id}/revoke",
    responses(
        (status = 204, description = "revoked"),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn revoke(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state.permissions.require(&state.db, &auth, "coupon:revoke").await?;
    let db = state.require_db()?;
    sqlx::query("UPDATE coupons SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL")
        .bind(id)
        .execute(db)
        .await
        .map_err(db_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/v1/admin/coupons — list coupons.
#[utoipa::path(
    get,
    path = "/api/v1/admin/coupons",
    responses(
        (status = 200, description = "coupon list", body = serde_json::Value),
        (status = 403, description = "permission denied", body = ErrorEnvelope),
    ),
    tag = "admin"
)]
pub async fn list(
    auth: AuthPrincipal,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, ApiError> {
    state.permissions.require(&state.db, &auth, "coupon:view").await?;
    let db = state.require_db()?;
    let rows = sqlx::query_as::<_, (Uuid, String, String, i32, i32, Option<chrono::DateTime<chrono::Utc>>, chrono::DateTime<chrono::Utc>)>(
        "SELECT id, code, action_type, max_uses, used_count, revoked_at, created_at
         FROM coupons ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    let items: Vec<Value> = rows
        .into_iter()
        .map(|(id, code, action_type, max_uses, used_count, revoked_at, created_at)| {
            json!({ "id": id, "code": code, "action_type": action_type, "max_uses": max_uses, "used_count": used_count, "revoked_at": revoked_at, "created_at": created_at })
        })
        .collect();
    Ok(Json(json!({ "items": items })))
}

fn generate_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let charset: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let len = 12;
    (0..len)
        .map(|_| {
            let idx = rng.gen_range(0..charset.len());
            charset[idx] as char
        })
        .collect()
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "coupon not found")
    } else {
        tracing::error!(error = %e, "coupons db error");
        ApiError::internal()
    }
}
