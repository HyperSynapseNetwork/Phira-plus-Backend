//! CSRF protection (contract §20, S-1 redesign).
//!
//! The old double-submit required the frontend to read a `ppb_csrf` cookie on
//! the API domain, which PPF/Panel cannot do cross-origin. New model:
//! - `GET /api/v1/me` returns a session-bound `csrf_token` (stateless HMAC of
//!   the session id under a derived server key).
//! - State-changing requests (POST/PUT/PATCH/DELETE) that authenticate via
//!   cookie must send `X-CSRF-Token: <token>` AND have an allowed `Origin`.
//! - Cookie SameSite=Lax; Bearer (Tauri) auth is exempt (no ambient authority).

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

use crate::app::AppState;
use crate::auth::jwt::decode_access;
use crate::auth::{ACCESS_COOKIE, REFRESH_COOKIE};
use crate::error::{ApiError, ErrorCode};
use crate::middleware::cookies;

type HmacSha256 = Hmac<Sha256>;

fn is_state_changing(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

/// Derive the CSRF HMAC key from the JWT secret (separate domain).
pub fn csrf_key(jwt_secret: &str) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(jwt_secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(b":ppb-csrf");
    mac.finalize().into_bytes().to_vec()
}

/// Session-bound CSRF token (stateless, derived from sid + server key).
pub fn csrf_token_for(sid: &Uuid, key: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(sid.as_bytes());
    let digest = mac.finalize().into_bytes();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, digest)
}

/// Constant-time string equality (same-length; no early return on first mismatch).
fn ct_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Whether the request authenticates via cookie (vs Bearer). Checks both the
/// access cookie and the refresh cookie (logout/refresh carry the latter).
fn uses_cookie_auth(req: &Request) -> bool {
    if req.headers().get(axum::http::header::AUTHORIZATION).is_some() {
        return false;
    }
    cookies::get_cookie(req.headers(), ACCESS_COOKIE).is_some()
        || cookies::get_cookie(req.headers(), REFRESH_COOKIE).is_some()
}

/// `Origin` must be absent (non-browser) or in the allowlist.
fn origin_allowed(origin: Option<&str>, allowed: &[String]) -> bool {
    match origin {
        Some(o) if !o.is_empty() => allowed.iter().any(|a| a == o),
        _ => true,
    }
}

/// Axum middleware enforcing the CSRF contract.
pub async fn csrf_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let cfg = &state.config.session;
    if is_state_changing(req.method()) && uses_cookie_auth(&req) {
        // 1) Origin must be allowed (defense-in-depth).
        let origin = req
            .headers()
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let allowed: Vec<String> = state
            .config
            .cors
            .allowed_origins
            .iter()
            .chain(state.config.cors.dev_origins.iter())
            .cloned()
            .collect();
        if !origin_allowed(origin.as_deref(), &allowed) {
            return Err(ApiError::new(ErrorCode::Auth, "CSRF origin rejected"));
        }

        // 2) Session-bound token must match X-CSRF-Token.
        let key = csrf_key(&state.secrets.jwt_secret);
        let token = cookies::get_cookie(req.headers(), ACCESS_COOKIE);
        if let Some(tok) = token {
            if let Ok(claims) = decode_access(&tok, &state.secrets.jwt_secret) {
                let expected = csrf_token_for(&claims.sid, &key);
                let provided = req
                    .headers()
                    .get(&cfg.csrf_header_name)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                if !ct_eq(&expected, provided) {
                    return Err(ApiError::new(
                        ErrorCode::Auth,
                        "CSRF token mismatch or missing X-CSRF-Token header",
                    ));
                }
            }
            // Invalid cookie is handled downstream by the auth extractor.
        }
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn token_is_deterministic_per_session() {
        let key = csrf_key("test-secret-test-secret-test-secret!!");
        let sid = Uuid::new_v4();
        let a = csrf_token_for(&sid, &key);
        let b = csrf_token_for(&sid, &key);
        assert_eq!(a, b);
        assert_ne!(a, csrf_token_for(&Uuid::new_v4(), &key));
        assert_ne!(a, csrf_token_for(&sid, &csrf_key("other-secret-other-secret!!")));
    }

    #[test]
    fn state_changing_methods() {
        assert!(is_state_changing(&Method::POST));
        assert!(is_state_changing(&Method::PUT));
        assert!(is_state_changing(&Method::PATCH));
        assert!(is_state_changing(&Method::DELETE));
        assert!(!is_state_changing(&Method::GET));
        assert!(!is_state_changing(&Method::HEAD));
        assert!(!is_state_changing(&Method::OPTIONS));
    }

    #[test]
    fn bearer_auth_skips_csrf() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer xyz".parse().unwrap(),
        );
        headers.insert(
            axum::http::header::COOKIE,
            format!("{ACCESS_COOKIE}=jwt").parse().unwrap(),
        );
        let mut req = axum::http::Request::builder()
            .method(Method::POST)
            .body(axum::body::Body::empty())
            .unwrap();
        *req.headers_mut() = headers;
        assert!(!uses_cookie_auth(&req));
    }

    #[test]
    fn cookie_auth_detected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::COOKIE,
            format!("{ACCESS_COOKIE}=jwt").parse().unwrap(),
        );
        let mut req = axum::http::Request::builder()
            .method(Method::POST)
            .body(axum::body::Body::empty())
            .unwrap();
        *req.headers_mut() = headers;
        assert!(uses_cookie_auth(&req));
    }

    #[test]
    fn origin_rules() {
        let allowed = vec!["https://phira.htadiy.com".to_string(), "http://localhost:3000".to_string()];
        assert!(origin_allowed(Some("https://phira.htadiy.com"), &allowed));
        assert!(!origin_allowed(Some("https://evil.example"), &allowed));
        assert!(origin_allowed(None, &allowed));
        assert!(origin_allowed(Some(""), &allowed));
    }
}
