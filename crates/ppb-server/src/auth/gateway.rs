//! PPB-owned standalone authentication gateway.
//!
//! PPB owns the credential/security runtime; visual language, localized strings,
//! error copy and legal-consent anatomy come from the small Auth Gateway subset
//! of the Frontend Design Contract. No PPF/Panel runtime dependency is required.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{Html, IntoResponse, Response};
use base64::Engine;
use serde::Deserialize;

use crate::app::AppState;
use crate::config::{LegalConfig, SecurityConfig, SiteConfig};

const TOKENS_JSON: &str = include_str!("../../../../contracts/auth-gateway/tokens.json");
const STRINGS_ZH_JSON: &str = include_str!("../../../../contracts/auth-gateway/strings.zh.json");
const STRINGS_EN_JSON: &str = include_str!("../../../../contracts/auth-gateway/strings.en.json");
const ERRORS_ZH_JSON: &str = include_str!("../../../../contracts/auth-gateway/errors.zh.json");
const ERRORS_EN_JSON: &str = include_str!("../../../../contracts/auth-gateway/errors.en.json");
const LOGO_BYTES: &[u8] = include_bytes!("../../../../contracts/auth-gateway/logo.png");

#[derive(Debug, Deserialize)]
pub struct LoginPageParams {
    #[serde(default)]
    pub return_to: Option<String>,
    #[serde(default)]
    pub client_type: Option<String>,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewayTokens {
    canvas: String,
    surface: String,
    surface_strong: String,
    border: String,
    text_primary: String,
    text_secondary: String,
    accent: String,
    accent_text: String,
    danger: String,
    focus: String,
    radius_control_px: u32,
    radius_window_px: u32,
    max_width_px: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewayStrings {
    document_title: String,
    product: String,
    title: String,
    subtitle: String,
    email: String,
    password: String,
    sign_in: String,
    github: String,
    or: String,
    consent_prefix: String,
    terms: String,
    privacy: String,
    consent_join: String,
    consent_required: String,
    legal_unavailable: String,
    network_error: String,
    generic_error: String,
    request_id: String,
    github_hint: String,
    client_ppf: String,
    client_panel: String,
    language: String,
    zh: String,
    en: String,
}

fn tokens() -> &'static GatewayTokens {
    static VALUE: OnceLock<GatewayTokens> = OnceLock::new();
    VALUE.get_or_init(|| serde_json::from_str(TOKENS_JSON).expect("auth gateway token contract must parse"))
}

fn strings(locale: &str) -> GatewayStrings {
    serde_json::from_str(if locale == "en" { STRINGS_EN_JSON } else { STRINGS_ZH_JSON })
        .expect("auth gateway string contract must parse")
}

fn error_strings(locale: &str) -> BTreeMap<String, String> {
    serde_json::from_str(if locale == "en" { ERRORS_EN_JSON } else { ERRORS_ZH_JSON })
        .expect("auth gateway error contract must parse")
}

/// Trusted frontend origin for a client identity. Native clients use the PPF
/// web origin for the current web gateway; native exchange-code work remains a
/// separate release gate.
fn trusted_origin<'a>(site: &'a SiteConfig, client_type: &str) -> &'a str {
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
                && v.len() <= 2048 => v,
        _ => "/",
    }
}

/// Resolve a relative product route against the trusted origin for the client.
pub fn resolve_redirect_target(
    site: &SiteConfig,
    security: &SecurityConfig,
    client_type: &str,
    return_to: Option<&str>,
) -> String {
    let origin = trusted_origin(site, client_type).trim_end_matches('/').to_string();
    match return_to {
        Some(v) if security.return_to_allowlist.iter().any(|u| u == v) => v.to_string(),
        Some(v) if v == safe_return_to(Some(v)) => format!("{origin}{v}"),
        _ => origin,
    }
}

fn legal_ready(legal: &LegalConfig) -> bool {
    legal.public_auth_enabled
        && !legal.terms_version.trim().is_empty()
        && !legal.privacy_version.trim().is_empty()
        && !legal.terms_url.trim().is_empty()
        && !legal.privacy_url.trim().is_empty()
}

fn resolve_legal_link(site: &SiteConfig, client_type: &str, value: &str) -> String {
    if value.starts_with("https://") || value.starts_with("http://localhost") || value.starts_with("http://127.0.0.1") {
        return value.to_string();
    }
    if value == safe_return_to(Some(value)) {
        return format!("{}{}", trusted_origin(site, client_type).trim_end_matches('/'), value);
    }
    "#".to_string()
}

fn locale_from(headers: &HeaderMap, requested: Option<&str>) -> &'static str {
    match requested {
        Some("en") | Some("en-US") | Some("en-GB") => return "en",
        Some("zh") | Some("zh-CN") | Some("zh-TW") => return "zh",
        _ => {}
    }
    let accept = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if accept.starts_with("en") { "en" } else { "zh" }
}

/// GET /auth/phira/login — standalone login HTML page.
pub async fn phira_login_page(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<LoginPageParams>,
) -> Response {
    let client_type = match params.client_type.as_deref() {
        Some("panel") => "panel",
        _ => "ppf",
    };
    let locale = locale_from(&headers, params.lang.as_deref());
    let return_to = safe_return_to(params.return_to.as_deref()).to_string();
    let redirect_to = resolve_redirect_target(
        &state.config.site,
        &state.config.security,
        client_type,
        params.return_to.as_deref(),
    );
    let nonce = new_csp_nonce();
    let html = login_html(
        &state,
        &return_to,
        &redirect_to,
        client_type,
        locale,
        &nonce,
        params.intent.as_deref() == Some("github"),
        params.error.as_deref(),
        params.request_id.as_deref(),
    );
    secure_gateway_response(html, &nonce)
}

fn login_html(
    state: &AppState,
    return_to: &str,
    redirect_to: &str,
    client_type: &str,
    locale: &str,
    nonce: &str,
    github_intent: bool,
    initial_error: Option<&str>,
    initial_request_id: Option<&str>,
) -> String {
    let t = tokens();
    let s = strings(locale);
    let errors = error_strings(locale);
    let client_label = if client_type == "panel" { &s.client_panel } else { &s.client_ppf };
    let subtitle = s.subtitle.replace("{client}", client_label);
    let ready = legal_ready(&state.config.legal);
    let github_ready = ready && state.secrets.github_configured();
    let terms_url = resolve_legal_link(&state.config.site, client_type, &state.config.legal.terms_url);
    let privacy_url = resolve_legal_link(&state.config.site, client_type, &state.config.legal.privacy_url);
    let logo = base64::engine::general_purpose::STANDARD.encode(LOGO_BYTES);
    let initial_error_text = initial_error
        .and_then(|code| errors.get(code))
        .cloned()
        .unwrap_or_default();
    let error_map = json_for_inline_script(&errors);
    let disabled = if ready { "" } else { " disabled" };
    let github_disabled = if github_ready { "" } else { " disabled" };
    let intent_class = if github_intent { " github-intent" } else { "" };

    format!(r#"<!DOCTYPE html>
<html lang="{locale}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="dark">
<title>{document_title}</title>
<style nonce="{nonce}">
:root{{--pp-canvas:{canvas};--pp-surface:{surface};--pp-surface-strong:{surface_strong};--pp-border:{border};--pp-text:{text};--pp-text-2:{text2};--pp-accent:{accent};--pp-accent-text:{accent_text};--pp-danger:{danger};--pp-focus:{focus};--pp-radius-control:{radius_control}px;--pp-radius-window:{radius_window}px}}
*{{box-sizing:border-box}} body{{margin:0;min-height:100vh;background:var(--pp-canvas);color:var(--pp-text);font-family:Inter,ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif;display:grid;place-items:center;padding:24px}}
.shell{{width:min({max_width}px,100%);position:relative}} .brand{{display:flex;align-items:center;justify-content:space-between;gap:16px;margin:0 2px 14px}} .brand img{{height:30px;width:auto}} .client{{font-size:12px;color:var(--pp-text-2);border:1px solid var(--pp-border);border-radius:999px;padding:5px 9px}}
.card{{background:var(--pp-surface);border:1px solid var(--pp-border);border-radius:var(--pp-radius-window);box-shadow:0 24px 70px rgba(0,0,0,.34);padding:26px}} h1{{font-size:24px;line-height:1.2;margin:0;font-weight:680;letter-spacing:-.02em}} .subtitle{{margin:8px 0 22px;color:var(--pp-text-2);font-size:14px;line-height:1.55}}
label.field{{display:block;margin:13px 0 6px;font-size:13px;color:var(--pp-text-2)}} input[type=email],input[type=password]{{width:100%;min-height:44px;padding:10px 12px;border-radius:var(--pp-radius-control);border:1px solid var(--pp-border);background:var(--pp-surface-strong);color:var(--pp-text);outline:none}} input:focus-visible,button:focus-visible,a:focus-visible{{outline:2px solid var(--pp-focus);outline-offset:2px}}
.action{{width:100%;min-height:44px;border-radius:var(--pp-radius-control);border:1px solid transparent;padding:10px 14px;font:inherit;font-weight:650;cursor:pointer;display:flex;align-items:center;justify-content:center;text-decoration:none}} .primary{{background:var(--pp-accent);color:var(--pp-accent-text)}} .secondary{{background:var(--pp-surface-strong);color:var(--pp-text);border-color:var(--pp-border)}} .action:disabled{{cursor:not-allowed;opacity:.45}} .github-intent .github{{border-color:color-mix(in srgb,var(--pp-accent) 55%,var(--pp-border))}}
.divider{{display:flex;align-items:center;gap:10px;color:var(--pp-text-2);font-size:12px;margin:15px 0}} .divider:before,.divider:after{{content:"";height:1px;flex:1;background:var(--pp-border)}}
.consent{{display:grid;grid-template-columns:24px 1fr;gap:8px;margin-top:16px;color:var(--pp-text-2);font-size:12px;line-height:1.55}} .consent input{{width:18px;height:18px;margin:2px 0 0}} a{{color:var(--pp-accent)}} .hint{{font-size:12px;color:var(--pp-text-2);line-height:1.5;margin:10px 0 0}} .legal-unavailable{{border:1px solid color-mix(in srgb,var(--pp-danger) 45%,var(--pp-border));background:color-mix(in srgb,var(--pp-danger) 9%,var(--pp-surface));border-radius:var(--pp-radius-control);padding:10px 12px;color:var(--pp-text);font-size:12px;line-height:1.55;margin-bottom:14px}}
.error{{color:var(--pp-danger);font-size:13px;margin-top:12px;min-height:20px;line-height:1.45}} .langs{{display:flex;justify-content:center;gap:4px;margin-top:14px}} .langs button{{min-width:44px;min-height:44px;border:0;background:transparent;color:var(--pp-text-2);border-radius:var(--pp-radius-control);cursor:pointer}} .langs button[aria-current=true]{{color:var(--pp-accent);background:var(--pp-surface-strong)}}
@media (prefers-reduced-motion: reduce){{*{{scroll-behavior:auto!important;transition:none!important;animation:none!important}}}} @media (prefers-reduced-transparency: reduce){{.card{{background:var(--pp-surface-strong);box-shadow:none}}}}
</style>
</head>
<body>
<main class="shell{intent_class}">
  <div class="brand"><img src="data:image/png;base64,{logo}" alt="{product}"><span class="client">{client_label}</span></div>
  <section class="card" aria-labelledby="gateway-title">
    <h1 id="gateway-title">{title}</h1><p class="subtitle">{subtitle}</p>
    {legal_block}
    <form id="login-form" aria-busy="false">
      <input type="hidden" name="return_to" value="{return_to}"><input type="hidden" id="redirect_to" value="{redirect_to}"><input type="hidden" name="client_type" value="{client_type}">
      <input type="hidden" name="terms_version" value="{terms_version}"><input type="hidden" name="privacy_version" value="{privacy_version}">
      <label class="field" for="email">{email}</label><input id="email" name="email" type="email" required autocomplete="email"{disabled}>
      <label class="field" for="password">{password}</label><input id="password" name="password" type="password" required autocomplete="current-password"{disabled}>
      <label class="consent"><input id="accepted" name="accepted" type="checkbox"{disabled}><span>{consent_prefix} <a href="{terms_url}" target="_blank" rel="noopener noreferrer">{terms}</a> {consent_join} <a href="{privacy_url}" target="_blank" rel="noopener noreferrer">{privacy}</a></span></label>
      <button class="action primary" id="submit" type="submit"{disabled}>{sign_in}</button>
      <div class="error" id="error" role="alert">{initial_error_text}</div>
    </form>
    <div class="divider">{or_text}</div>
    <button class="action secondary github" id="github" type="button"{github_disabled}>{github}</button>
    <p class="hint">{github_hint}</p>
  </section>
  <div class="langs" aria-label="{language}"><button type="button" data-lang="zh" aria-current="{zh_current}">{zh_label}</button><button type="button" data-lang="en" aria-current="{en_current}">{en_label}</button></div>
</main>
<script nonce="{nonce}">
(function(){{
  const form=document.getElementById('login-form'), err=document.getElementById('error'), github=document.getElementById('github'), submit=document.getElementById('submit');
  const messages={error_map};
  const generic={generic_json}, network={network_json}, consentRequired={consent_required_json}, requestIdLabel={request_id_json};
  const legalReady={legal_ready_json}, githubReady={github_ready_json};
  let busy=false;
  function setBusy(value){{busy=value;form.setAttribute('aria-busy',value?'true':'false');form.email.disabled=value||!legalReady;form.password.disabled=value||!legalReady;form.accepted.disabled=value||!legalReady;submit.disabled=value||!legalReady;github.disabled=value||!githubReady}}
  function showApiError(j){{const e=j&&j.error?j.error:null;let text=(e&&messages[e.code])||generic;if(e&&e.request_id)text+=' · '+requestIdLabel+': '+e.request_id;err.textContent=text;if(e&&e.code==='AUTH_LEGAL_CONSENT_REQUIRED'&&form.accepted)form.accepted.focus()}}
  form.addEventListener('submit',async function(e){{e.preventDefault();if(busy)return;err.textContent='';const body={{email:form.email.value,password:form.password.value,client_type:form.client_type.value,return_to:form.return_to.value,accepted:!!(form.accepted&&form.accepted.checked),terms_version:form.terms_version.value,privacy_version:form.privacy_version.value}};setBusy(true);try{{const res=await fetch('/api/v1/auth/phira/login',{{method:'POST',headers:{{'Content-Type':'application/json'}},credentials:'same-origin',body:JSON.stringify(body)}});if(res.ok){{window.location.href=document.getElementById('redirect_to').value||'/';return}}showApiError(await res.json().catch(()=>({{}})))}}catch(_e){{err.textContent=network}}finally{{setBusy(false)}}}});
  github.addEventListener('click',function(){{if(busy)return;err.textContent='';setBusy(true);const q=new URLSearchParams({{return_to:form.return_to.value,client_type:form.client_type.value,accepted:String(!!(form.accepted&&form.accepted.checked)),terms_version:form.terms_version.value,privacy_version:form.privacy_version.value}});window.location.href='/api/v1/auth/github/login/start?'+q.toString()}});
  document.querySelectorAll('[data-lang]').forEach(function(btn){{btn.addEventListener('click',function(){{const u=new URL(window.location.href);u.searchParams.set('lang',btn.dataset.lang);window.location.href=u.toString()}})}});
}})();
</script>
</body></html>"#,
        locale=locale, nonce=nonce,
        document_title=html_escape(&s.document_title), canvas=t.canvas, surface=t.surface, surface_strong=t.surface_strong,
        border=t.border, text=t.text_primary, text2=t.text_secondary, accent=t.accent, accent_text=t.accent_text,
        danger=t.danger, focus=t.focus, radius_control=t.radius_control_px, radius_window=t.radius_window_px, max_width=t.max_width_px,
        intent_class=intent_class, logo=logo, product=html_escape(&s.product), client_label=html_escape(client_label), title=html_escape(&s.title), subtitle=html_escape(&subtitle),
        legal_block=if ready { String::new() } else { format!("<div class=\"legal-unavailable\">{}</div>", html_escape(&s.legal_unavailable)) },
        return_to=html_escape(return_to), redirect_to=html_escape(redirect_to), client_type=html_escape(client_type),
        terms_version=html_escape(&state.config.legal.terms_version), privacy_version=html_escape(&state.config.legal.privacy_version),
        email=html_escape(&s.email), password=html_escape(&s.password), disabled=disabled,
        consent_prefix=html_escape(&s.consent_prefix), terms_url=html_escape(&terms_url), terms=html_escape(&s.terms), consent_join=html_escape(&s.consent_join), privacy_url=html_escape(&privacy_url), privacy=html_escape(&s.privacy),
        sign_in=html_escape(&s.sign_in), initial_error_text=html_escape(&initial_error_text), or_text=html_escape(&s.or), github_disabled=github_disabled, github=html_escape(&s.github), github_hint=html_escape(&s.github_hint),
        language=html_escape(&s.language), zh_current=if locale=="zh"{"true"}else{"false"}, en_current=if locale=="en"{"true"}else{"false"}, zh_label=html_escape(&s.zh), en_label=html_escape(&s.en),
        error_map=error_map, generic_json=json_for_inline_script(&s.generic_error), network_json=json_for_inline_script(&s.network_error), consent_required_json=json_for_inline_script(&s.consent_required), request_id_json=json_for_inline_script(&s.request_id), legal_ready_json=if ready {"true"} else {"false"}, github_ready_json=if github_ready {"true"} else {"false"},
    )
}

fn new_csp_nonce() -> String {
    let mut bytes = [0u8; 18];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn secure_gateway_response(html: String, nonce: &str) -> Response {
    let mut response = Html(html).into_response();
    let headers = response.headers_mut();
    headers.insert(axum::http::header::CACHE_CONTROL, HeaderValue::from_static("no-store, max-age=0"));
    headers.insert(HeaderName::from_static("pragma"), HeaderValue::from_static("no-cache"));
    headers.insert(HeaderName::from_static("referrer-policy"), HeaderValue::from_static("no-referrer"));
    headers.insert(HeaderName::from_static("x-content-type-options"), HeaderValue::from_static("nosniff"));
    headers.insert(HeaderName::from_static("x-frame-options"), HeaderValue::from_static("DENY"));
    headers.insert(HeaderName::from_static("permissions-policy"), HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()"));
    let csp = format!(
        "default-src 'none'; img-src 'self' data:; style-src 'nonce-{nonce}'; script-src 'nonce-{nonce}'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'; object-src 'none'"
    );
    if let Ok(value) = HeaderValue::from_str(&csp) {
        headers.insert(HeaderName::from_static("content-security-policy"), value);
    }
    response
}

fn json_for_inline_script<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value)
        .expect("auth gateway inline JSON must serialize")
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
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
        SiteConfig { ppf_url: "https://phira.htadiy.com".to_string(), panel_url: "https://panel-phira.htadiy.com".to_string(), docs_url: "https://docs.phira.htadiy.com".to_string(), visit_count: 0 }
    }
    fn security() -> SecurityConfig {
        SecurityConfig { return_to_allowlist: vec!["https://phira.htadiy.com".to_string(), "https://panel-phira.htadiy.com".to_string()] }
    }
    #[test] fn safe_relative_accepted(){ assert_eq!(safe_return_to(Some("/rooms")), "/rooms"); }
    #[test] fn unsafe_rejected_to_root(){ assert_eq!(safe_return_to(Some("https://evil.com")), "/"); assert_eq!(safe_return_to(Some("//evil.com")), "/"); }
    #[test] fn relative_joins_trusted_origins(){ let s=site(); let sec=security(); assert_eq!(resolve_redirect_target(&s,&sec,"ppf",Some("/rooms")),"https://phira.htadiy.com/rooms"); assert_eq!(resolve_redirect_target(&s,&sec,"panel",Some("/users")),"https://panel-phira.htadiy.com/users"); }
}
