//! Cookie helpers (no tower-cookies — avoids layer ordering coupling with CSRF).
//!
//! Cookie policy (see PHASE_A_PLAN P2): Secure + HttpOnly access cookie,
//! host-only domain `api-phira.htadiy.com`, SameSite=Lax (same-site cross-origin
//! credentialed fetch from PPF/Panel). CSRF cookie is non-HttpOnly.

use axum::http::header;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use cookie::{Cookie, SameSite};

/// Options controlling cookie serialization.
#[derive(Debug, Clone)]
pub struct CookieOpts {
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: SameSite,
}

impl CookieOpts {
    pub fn new(domain: &str) -> Self {
        Self {
            domain: domain.to_string(),
            path: "/".to_string(),
            secure: true,
            http_only: true,
            same_site: SameSite::Lax,
        }
    }

    pub fn http_only(mut self, v: bool) -> Self {
        self.http_only = v;
        self
    }

    pub fn same_site(mut self, v: SameSite) -> Self {
        self.same_site = v;
        self
    }
}

/// Serialize a Set-Cookie header value.
pub fn set_cookie(name: &str, value: &str, opts: &CookieOpts, max_age_secs: i64) -> HeaderValue {
    let mut builder = Cookie::build(name.to_string(), value.to_string())
        .domain(opts.domain.clone())
        .path(opts.path.clone())
        .secure(opts.secure)
        .http_only(opts.http_only)
        .same_site(opts.same_site);
    if max_age_secs > 0 {
        builder = builder.max_age(cookie::time::Duration::seconds(max_age_secs));
    }
    HeaderValue::from_str(&builder.finish().to_string())
        .expect("cookie serializes to a valid header value")
}

/// Serialize a Set-Cookie header value that deletes the cookie immediately.
pub fn clear_cookie(name: &str, opts: &CookieOpts) -> HeaderValue {
    let builder = Cookie::build(name.to_string(), String::new())
        .domain(opts.domain.clone())
        .path(opts.path.clone())
        .secure(opts.secure)
        .http_only(opts.http_only)
        .same_site(opts.same_site)
        .max_age(cookie::time::Duration::ZERO);
    HeaderValue::from_str(&builder.finish().to_string())
        .expect("cookie serializes to a valid header value")
}

/// Read a cookie by name from request headers.
pub fn get_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let value = headers.get(header::COOKIE)?.to_str().ok()?;
    Cookie::split_parse(value)
        .filter_map(Result::ok)
        .find(|c| c.name() == name)
        .map(|c| c.value().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_cookie() {
        let opts = CookieOpts::new("api-phira.htadiy.com");
        let header = set_cookie("ppb_access", "abc", &opts, 3600);
        let mut headers = HeaderMap::new();
        headers.insert(header::SET_COOKIE, header);
        // Build a Cookie request header manually.
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("ppb_access=abc; ppb_csrf=tok"),
        );
        assert_eq!(get_cookie(&headers, "ppb_access").as_deref(), Some("abc"));
        assert_eq!(get_cookie(&headers, "ppb_csrf").as_deref(), Some("tok"));
    }

    #[test]
    fn cookie_has_secure_http_only() {
        let opts = CookieOpts::new("api-phira.htadiy.com");
        let header = set_cookie("ppb_access", "v", &opts, 60).to_str().unwrap().to_string();
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));
        assert!(header.contains("Domain=api-phira.htadiy.com"));
    }

    #[test]
    fn csrf_cookie_not_http_only() {
        let opts = CookieOpts::new("api-phira.htadiy.com").http_only(false);
        let header = set_cookie("ppb_csrf", "t", &opts, 60).to_str().unwrap().to_string();
        assert!(!header.contains("HttpOnly"));
    }
}
