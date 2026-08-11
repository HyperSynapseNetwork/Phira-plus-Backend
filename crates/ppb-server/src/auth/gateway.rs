//! PPB Auth gateway HTML page (design §6.6, contract P13).
//!
//! `GET https://api-phira.htadiy.com/auth/phira/login?return_to=<relative>` serves
//! a self-contained login page. `return_to` is validated to be a safe relative
//! path (anti open-redirect) and re-validated server-side by the login API.
//! The page is separate from the `/api/v1/auth/*` JSON API.

use axum::extract::Query;
use axum::response::Html;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LoginPageParams {
    #[serde(default)]
    pub return_to: Option<String>,
    #[serde(default)]
    pub client_type: Option<String>,
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

/// GET /auth/phira/login — login HTML page.
pub async fn phira_login_page(Query(params): Query<LoginPageParams>) -> Html<String> {
    let return_to = safe_return_to(params.return_to.as_deref());
    let client_type = params.client_type.unwrap_or_else(|| "ppf".to_string());
    let page = login_html(return_to, &client_type);
    Html(page)
}

fn login_html(return_to: &str, client_type: &str) -> String {
    // Values are escaped for HTML attribute context.
    let return_esc = html_escape(return_to);
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
  button {{ margin-top:16px; width:100%; padding:10px; border:0; border-radius:8px; background:#00d4ff; color:#04222b; font-weight:600; cursor:pointer; }}
  .error {{ color:#ff7a7a; font-size:13px; margin-top:10px; min-height:16px; }}
  .hint {{ font-size:12px; color:#9aa0b8; margin-top:12px; }}
</style>
</head>
<body>
<div class="card">
  <h1>Phira+ 登录</h1>
  <form id="login-form">
    <input type="hidden" name="return_to" value="{return_esc}">
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
        var target = form.return_to.value || '/';
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
    fn html_escaped() {
        let out = login_html("/?a=\"x\"&b=<y>", "ppf");
        assert!(out.contains("value=\"/?a=&quot;x&quot;&amp;b=&lt;y&gt;\""));
    }
}
