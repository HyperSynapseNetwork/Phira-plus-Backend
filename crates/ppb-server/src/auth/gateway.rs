//! PPB Auth gateway HTML page (design §6.6, contract P13).
//!
//! `GET https://api-phira.htadiy.com/auth/phira/login?return_to=<relative>` serves
//! a self-contained login page. `return_to` is validated to be a safe relative
//! path (anti open-redirect) and re-validated server-side by the login API.
//! The page is separate from the `/api/v1/auth/*` JSON API.
//!
//! The redirect target is resolved **server-side against a trusted frontend
//! origin** chosen by the client identity (`client_type`): PPF → `site.ppf_url`,
//! Panel → `site.panel_url`. A relative `return_to` such as `/rooms` is joined
//! onto that origin (`https://phira.htadiy.com/rooms`) — it is never interpreted
//! against the API domain. Absolute URLs are only accepted when they exactly
//! match an entry in `security.return_to_allowlist` (open-redirect guard).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Html;
use serde::Deserialize;

use crate::app::AppState;
use crate::config::{SecurityConfig, SiteConfig};

#[derive(Debug, Deserialize)]
pub struct LoginPageParams {
    #[serde(default)]
    pub return_to: Option<String>,
    #[serde(default)]
    pub client_type: Option<String>,
}

/// Trusted frontend origin for a client identity. Unknown clients default to PPF.
fn trusted_origin(site: &SiteConfig, client_type: &str) -> &str {
    match client_type {
        "panel" => &site.panel_url,
        _ => &site.ppf_url,
    }
}

/// A `return_to` value safe to reflect into the page redirect target.
pub fn safe_return_to(value: Option<&str>) -> &str {
    match value {
        Some(v)
            if v.starts_with('/')
                && !v.starts_with("//")
                && !v.contains('\\')
                && !v.contains('@')
                && !v.contains("://")
                && v.len() <= 2048 =>
        {
            v
        }
        _ => "/",
    }
}

/// Resolve the final redirect target server-side.
///
/// - A safe relative `return_to` is joined onto the trusted frontend origin for
///   the given client identity, so `/rooms` becomes `https://phira.htadiy.com/rooms`
///   — never the API domain.
/// - An absolute URL is accepted only when it exactly matches a whitelisted
///   origin (`security.return_to_allowlist`); anything else falls back to the
///   trusted origin root (open-redirect guard).
pub fn resolve_redirect_target(
    site: &SiteConfig,
    security: &SecurityConfig,
    client_type: &str,
    return_to: Option<&str>,
) -> String {
    let origin = trusted_origin(site, client_type).trim_end_matches('/').to_string();
    match return_to {
        // Exact whitelisted absolute origin (e.g. `https://phira.htadiy.com`).
        Some(v) if security.return_to_allowlist.iter().any(|u| u == v) => v.to_string(),
        // Safe relative path → join onto the trusted frontend origin.
        Some(v) if v == safe_return_to(Some(v)) => format!("{origin}{v}"),
        // Missing / malformed / absolute-not-in-allowlist → origin root.
        _ => origin,
    }
}

/// GET /auth/phira/login — login HTML page.
pub async fn phira_login_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LoginPageParams>,
) -> Html<String> {
    let client_type = params.client_type.as_deref().unwrap_or("ppf").to_string();
    // The hidden `return_to` posted to the JSON API stays a safe relative path;
    // the JS redirect target is the server-resolved full URL.
    let return_to = safe_return_to(params.return_to.as_deref()).to_string();
    let redirect_to = resolve_redirect_target(
        &state.config.site,
        &state.config.security,
        &client_type,
        params.return_to.as_deref(),
    );
    let page = login_html(&return_to, &redirect_to, &client_type);
    Html(page)
}

fn login_html(return_to: &str, redirect_to: &str, client_type: &str) -> String {
    // Values are escaped for HTML attribute context.
    let return_esc = html_escape(return_to);
    let redirect_esc = html_escape(redirect_to);
    let client_esc = html_escape(client_type);
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Phira+ 登录</title>
<style>
  body {{ font-family: system-ui, sans-serif; background:#0f1220; color:#e8eaf2; display:flex; align-items:center; justify-content:center; min-height:100vh; margin:0; }}
  .card {{ background:#171a2b; border:1px solid #262a40; border-radius:12px; padding:28px; width:320px; }}
  h1 {{ font-size:18px; margin:0 0 16px; }}
  label {{ display:block; margin:10px 0 4px; font-size:13px; }}
  input {{ width:100%; box-sizing:border-box; padding:9px 10px; border-radius:8px; border:1px solid #33385a; background:#0f1220; color:#e8eaf2; }}
  button {{ margin-top:16px; width:100%; padding:10px; border-radius:8px; background:#00d4ff; color:#04222b; font-weight:600; cursor:pointer; }}
  .error {{ color:#ff7a7a; font-size:13px; margin-top:10px; min-height:16px; }}
  .hint {{ font-size:12px; color:#9aa0b8; margin-top:12px; }}
</style>
</head>
<body>
<div class="card">
  <h1>Phira+ 登录</h1>
  <form id="login-form">
    <input type="hidden" name="return_to" value="{return_esc}">
    <input type="hidden" id="redirect_to" name="redirect_to" value="{redirect_esc}">
    <input type="hidden" name="client_type" value="{client_esc}">
    <label for="email">Phira 邮箱</label>
    <input id="email" name="email" type="email" required autocomplete="email">
    <label for="password">密码</label>
    <input id="password" name="password" type="password" required autocomplete="current-password">
    <button type="submit">登录</button>
    <div class="error" id="error"></div>
  </form>
  <div class="hint">登录即代表同意用户协议与隐私政策。</div>
</div>
<script>
(function(){{
  var form = document.getElementById('login-form');
  form.addEventListener('submit', async function(e){{
    e.preventDefault();
    var err = document.getElementById('error');
    err.textContent = '';
    var body = {{
      email: form.email.value,
      password: form.password.value,
      client_type: form.client_type.value,
      return_to: form.return_to.value
    }};
    try {{
      var res = await fetch('/api/v1/auth/phira/login', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        credentials: 'same-origin',
        body: JSON.stringify(body)
      }});
      if (res.ok) {{
        var target = document.getElementById('redirect_to').value || form.return_to.value || '/';
        window.location.href = target;
      }} else {{
        var j = await res.json().catch(function(){{ return {{}}; }});
        err.textContent = (j && j.error && j.error.message) ? j.error.message : ('登录失败 (' + res.status + ')');
      }}
    }} catch (ex) {{
      err.textContent = '网络错误，请重试';
    }}
  }});
}})();
</script>
</body>
</html>"#
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{SecurityConfig, SiteConfig};

    fn site() -> SiteConfig {
        SiteConfig {
            ppf_url: "https://phira.htadiy.com".to_string(),
            panel_url: "https://panel-phira.htadiy.com".to_string(),
            docs_url: "https://docs.phira.htadiy.com".to_string(),
            visit_count: 0,
        }
    }

    fn security() -> SecurityConfig {
        SecurityConfig {
            return_to_allowlist: vec![
                "https://phira.htadiy.com".to_string(),
                "https://panel-phira.htadiy.com".to_string(),
            ],
        }
    }

    #[test]
    fn safe_relative_accepted() {
        assert_eq!(safe_return_to(Some("/rooms")), "/rooms");
        assert_eq!(safe_return_to(Some("/users/123")), "/users/123");
    }

    #[test]
    fn unsafe_rejected_to_root() {
        assert_eq!(safe_return_to(Some("https://evil.com")), "/");
        assert_eq!(safe_return_to(Some("//evil.com")), "/");
        assert_eq!(safe_return_to(Some("\\\\evil.com")), "/");
        assert_eq!(safe_return_to(Some("javascript:alert(1)")), "/");
        assert_eq!(safe_return_to(None), "/");
    }

    #[test]
    fn relative_joins_trusted_ppf_origin() {
        let s = site();
        let sec = security();
        assert_eq!(
            resolve_redirect_target(&s, &sec, "ppf", Some("/rooms")),
            "https://phira.htadiy.com/rooms"
        );
        assert_eq!(
            resolve_redirect_target(&s, &sec, "ppf", Some("/users/123")),
            "https://phira.htadiy.com/users/123"
        );
    }

    #[test]
    fn relative_joins_trusted_panel_origin() {
        let s = site();
        let sec = security();
        assert_eq!(
            resolve_redirect_target(&s, &sec, "panel", Some("/users/1")),
            "https://panel-phira.htadiy.com/users/1"
        );
    }

    #[test]
    fn never_resolves_relative_to_api_domain() {
        let s = site();
        let sec = security();
        let target = resolve_redirect_target(&s, &sec, "ppf", Some("/rooms"));
        assert!(!target.starts_with("https://api-phira.htadiy.com"));
    }

    #[test]
    fn absolute_only_when_whitelisted() {
        let s = site();
        let sec = security();
        assert_eq!(
            resolve_redirect_target(&s, &sec, "ppf", Some("https://phira.htadiy.com")),
            "https://phira.htadiy.com"
        );
        // Absolute non-whitelisted → falls back to trusted origin root.
        assert_eq!(
            resolve_redirect_target(&s, &sec, "ppf", Some("https://evil.com/steal")),
            "https://phira.htadiy.com"
        );
        // Protocol-relative → falls back to trusted origin root.
        assert_eq!(
            resolve_redirect_target(&s, &sec, "ppf", Some("//evil.com")),
            "https://phira.htadiy.com"
        );
    }

    #[test]
    fn missing_returns_origin_root() {
        let s = site();
        let sec = security();
        assert_eq!(resolve_redirect_target(&s, &sec, "ppf", None), "https://phira.htadiy.com");
        assert_eq!(
            resolve_redirect_target(&s, &sec, "panel", Some("")),
            "https://panel-phira.htadiy.com"
        );
    }

    #[test]
    fn html_escaped() {
        let out = login_html("/?a=\"x\"&b=<y>", "https://phira.htadiy.com/?a=\"x\"", "ppf");
        assert!(out.contains("value=\"/?a=&quot;x&quot;&amp;b=&lt;y&gt;\""));
        assert!(out.contains("value=\"https://phira.htadiy.com/?a=&quot;x&quot;\""));
    }
}
