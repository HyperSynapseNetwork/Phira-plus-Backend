//! Auth extractor: turns a request into an `AuthPrincipal` (cookie or Bearer).

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::header;
use axum::http::request::Parts;

use crate::app::AppState;
use crate::auth::jwt::decode_access;
use crate::auth::types::{AuthPrincipal, ClientType, PrincipalType};
use crate::auth::ACCESS_COOKIE;
use crate::error::{ApiError, ErrorCode};
use crate::middleware::cookies;

/// Extract an authenticated principal from cookie (`ppb_access`) or
/// `Authorization: Bearer <jwt>` (Tauri).
impl FromRequestParts<Arc<AppState>> for AuthPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_or_cookie(&parts.headers, ACCESS_COOKIE)
            .ok_or_else(|| ApiError::new(ErrorCode::Session, "missing credentials"))?;

        let claims = decode_access(&token, &state.secrets.jwt_secret)?;

        // Best-effort session liveness check when a DB is configured.
        if let Some(db) = &state.db {
            let active: (bool,) = sqlx::query_as::<_, (bool,)>(
                "SELECT EXISTS(
                    SELECT 1 FROM sessions
                    WHERE id = $1 AND revoked_at IS NULL AND expires_at > now()
                 )",
            )
            .bind(claims.sid)
            .fetch_one(db)
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "session liveness check failed");
                ApiError::internal()
            })?;
            if !active.0 {
                return Err(ApiError::new(ErrorCode::Session, "session inactive"));
            }
        }

        Ok(AuthPrincipal {
            sub: claims.sub,
            sid: claims.sid,
            principal_type: claims.principal_type,
            client_type: claims.client_type,
            request_id: crate::middleware::request_id::read_request_id(&parts.headers),
        })
    }
}

/// Return the JWT from `Authorization: Bearer` or the access cookie.
pub fn extract_bearer_or_cookie(headers: &header::HeaderMap, cookie_name: &str) -> Option<String> {
    if let Some(authz) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = authz.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }
    cookies::get_cookie(headers, cookie_name)
}

/// Convenience: build a principal for tests (no DB).
#[allow(dead_code)]
pub fn principal_for_test(
    sub: uuid::Uuid,
    sid: uuid::Uuid,
    principal_type: PrincipalType,
    client_type: ClientType,
) -> AuthPrincipal {
    AuthPrincipal {
        sub,
        sid,
        principal_type,
        client_type,
        request_id: "test-request".to_string(),
    }
}
