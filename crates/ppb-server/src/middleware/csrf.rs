//! CSRF double-submit protection for cookie-authenticated state-changing requests.
//!
//! Flow: the auth/login endpoints set a non-HttpOnly `ppb_csrf` cookie AND the
//! HttpOnly `ppb_access` cookie. For any state-changing (POST/PUT/PATCH/DELETE)
//! request that authenticates via cookie, the `X-CSRF-Token` header must match
//! the `ppb_csrf` cookie value. Bearer (Tauri) auth is exempt (no ambient
//! authority). See design §6.4 and §27.3.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::Response;

use crate::app::AppState;
use crate::auth::ACCESS_COOKIE;
use crate::error::{ApiError, ErrorCode};
use crate::middleware::cookies;

fn is_state_changing(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

/// Detect whether this request authenticates via cookie (vs Bearer).
fn uses_cookie_auth(req: &Request, csrf_cookie: &str) -> bool {
    if req.headers().get(axum::http::header::AUTHORIZATION).is_some() {
        return false;
    }
    cookies::get_cookie(req.headers(), ACCESS_COOKIE).is_some()
        || cookies::get_cookie(req.headers(), csrf_cookie).is_some()
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

/// Pure double-submit check (unit-testable).
fn csrf_valid(cookie: Option<&str>, header: Option<&str>) -> bool {
    match (cookie, header) {
        (Some(c), Some(h)) if !c.is_empty() && !h.is_empty() => ct_eq(c, h),
        _ => false,
    }
}

/// Axum middleware enforcing the double-submit rule.
pub async fn csrf_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let cfg = &state.config.session;
    if is_state_changing(req.method()) && uses_cookie_auth(&req, &cfg.csrf_cookie_name) {
        let cookie_val = cookies::get_cookie(req.headers(), &cfg.csrf_cookie_name);
        let header_val = req
            .headers()
            .get(&cfg.csrf_header_name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        if !csrf_valid(cookie_val.as_deref(), header_val.as_deref()) {
            return Err(ApiError::new(
                ErrorCode::Auth,
                "CSRF token mismatch or missing X-CSRF-Token header",
            ));
        }
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn valid_when_matching() {
        assert!(csrf_valid(Some("tok"), Some("tok")));
    }

    #[test]
    fn invalid_on_mismatch_or_missing() {
        assert!(!csrf_valid(Some("tok"), Some("other")));
        assert!(!csrf_valid(Some("tok"), None));
        assert!(!csrf_valid(None, Some("tok")));
        assert!(!csrf_valid(Some(""), Some("")));
        assert!(!csrf_valid(None, None));
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
        assert!(!uses_cookie_auth(&req, "ppb_csrf"));
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
        assert!(uses_cookie_auth(&req, "ppb_csrf"));
    }
}
