//! PPB configuration.
//!
//! Two layers (design §20.1):
//! - **Deployment / secrets**: env vars / secret file (never logged, never returned by Panel).
//! - **Runtime config**: TOML file (config/ppb.toml or PPB_RUNTIME_CONFIG), serde-deserialized.

pub mod deployment;
pub mod pmp;
pub mod repo;
pub mod routes;

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Top-level runtime config, mirroring `config/example.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeConfig {
    pub server: ServerConfig,
    pub site: SiteConfig,
    pub cors: CorsConfig,
    pub session: SessionConfig,
    pub pmp: PmpConfig,
    pub phira: PhiraConfig,
    pub rate_limit: RateLimitConfig,
    pub audit: AuditConfig,
    pub notifications: NotificationConfig,
    pub metrics: MetricsConfig,
    pub security: SecurityConfig,
    pub github: GithubConfig,
    pub legal: LegalConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub public_url: String,
    pub graceful_shutdown_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8080".parse().expect("valid default addr"),
            public_url: "https://api-phira.htadiy.com".to_string(),
            graceful_shutdown_secs: 15,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SiteConfig {
    pub ppf_url: String,
    pub panel_url: String,
    pub docs_url: String,
    /// Privacy-friendly aggregate visit count baseline (P-86). A server-side
    /// in-memory counter adds to this; without client fingerprinting the value
    /// comes from config/counting, and defaults to 0 when unset.
    pub visit_count: i64,
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            ppf_url: "https://phira.htadiy.com".to_string(),
            panel_url: "https://panel-phira.htadiy.com".to_string(),
            docs_url: "https://docs.phira.htadiy.com".to_string(),
            visit_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CorsConfig {
    pub credentials: bool,
    pub allowed_origins: Vec<String>,
    pub dev_origins: Vec<String>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            credentials: true,
            allowed_origins: vec![
                "https://phira.htadiy.com".to_string(),
                "https://panel-phira.htadiy.com".to_string(),
            ],
            dev_origins: vec![
                "http://localhost:3000".to_string(),
                "http://localhost:5173".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub access_ttl_secs: i64,
    pub refresh_ttl_secs: i64,
    pub cookie_domain: String,
    pub cookie_secure: bool,
    pub cookie_samesite: String, // lax | strict | none
    pub csrf_cookie_name: String,
    pub csrf_header_name: String,
    pub reauth_ttl_secs: i64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            access_ttl_secs: 3600,
            refresh_ttl_secs: 2592000,
            // Empty means host-only, which is both safer and portable across
            // official and self-hosted API domains.
            cookie_domain: String::new(),
            cookie_secure: true,
            cookie_samesite: "lax".to_string(),
            csrf_cookie_name: "ppb_csrf".to_string(),
            csrf_header_name: "X-CSRF-Token".to_string(),
            reauth_ttl_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PmpConfig {
    pub openuds_path: PathBuf,
    pub auth_mode: String, // token | approve
    pub client_name: String,
    pub reconnect_base_ms: u64,
    pub reconnect_max_ms: u64,
    pub request_timeout_ms: u64,
    pub capabilities: Vec<String>,
    /// Path to PMP `server_config.yml` for Form Descriptor snapshot/rollback.
    pub config_path: Option<PathBuf>,
    /// PMP HTTP health URL (e.g. `http://127.0.0.1:12347`) for health checks.
    pub http_url: Option<String>,
}

impl Default for PmpConfig {
    fn default() -> Self {
        Self {
            openuds_path: PathBuf::from("/var/run/pmp-openuds.sock"),
            auth_mode: "approve".to_string(),
            client_name: "ppb-server".to_string(),
            reconnect_base_ms: 500,
            reconnect_max_ms: 30000,
            request_timeout_ms: 10000,
            capabilities: vec![
                "persist.touches".to_string(),
                "persist.judges".to_string(),
                "persist.rounds".to_string(),
                "room.chat_send".to_string(),
                "stream.touches".to_string(),
                "stream.judges".to_string(),
            ],
            config_path: None,
            http_url: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PhiraConfig {
    pub base_url: String,
    pub timeout_ms: u64,
    pub access_token_ttl_secs: i64,
    /// Data gateway cache TTL for public data.
    pub gateway_ttl_secs: i64,
    /// Data gateway rate limit (requests/min).
    pub gateway_rate_per_minute: u32,
    /// Aggregator (TopChart hourly snapshots).
    pub aggregator_enabled: bool,
    pub aggregator_interval_hours: u64,
    pub aggregator_top_n: i32,
}

impl Default for PhiraConfig {
    fn default() -> Self {
        Self {
            base_url: "https://phira.5wyxi.com".to_string(),
            timeout_ms: 15000,
            access_token_ttl_secs: 21600,
            gateway_ttl_secs: 120,
            gateway_rate_per_minute: 60,
            aggregator_enabled: true,
            aggregator_interval_hours: 1,
            aggregator_top_n: 50,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    pub login_per_minute: u32,
    pub reauth_per_minute: u32,
    pub github_start_per_minute: u32,
    pub github_callback_per_minute: u32,
    pub github_provider_per_minute: u32,
    pub chat_send_per_minute: u32,
    pub social_per_minute: u32,
    pub raw_cli_per_minute: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            login_per_minute: 10,
            reauth_per_minute: 10,
            github_start_per_minute: 10,
            github_callback_per_minute: 20,
            github_provider_per_minute: 120,
            chat_send_per_minute: 60,
            social_per_minute: 30,
            raw_cli_per_minute: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    pub retention_days: i32,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self { retention_days: 90 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationConfig {
    pub default_chat_channel: String,
    /// Web Push VAPID private key (P-256, PEM) and subject. When absent the
    /// WebPush adapter returns `not_configured` (Panel marks it 待配).
    pub vapid_private_key_pem: Option<String>,
    pub vapid_subject: Option<String>,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            default_chat_channel: "only_when_companion_background".to_string(),
            vapid_private_key_pem: None,
            vapid_subject: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MetricsConfig {
    pub retention_days: i32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { retention_days: 30 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub return_to_allowlist: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            return_to_allowlist: vec![
                "https://phira.htadiy.com".to_string(),
                "https://panel-phira.htadiy.com".to_string(),
            ],
        }
    }
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LegalConfig {
    /// Public user authentication stays disabled until approved legal documents are configured.
    pub public_auth_enabled: bool,
    pub terms_version: String,
    pub privacy_version: String,
    pub terms_url: String,
    pub privacy_url: String,
}

impl Default for LegalConfig {
    fn default() -> Self {
        Self {
            public_auth_enabled: false,
            terms_version: String::new(),
            privacy_version: String::new(),
            terms_url: String::new(),
            privacy_url: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GithubConfig {
    pub callback_url: String,
}

impl Default for GithubConfig {
    fn default() -> Self {
        Self {
            callback_url: "https://api-phira.htadiy.com/api/v1/auth/github/callback".to_string(),
        }
    }
}

impl RuntimeConfig {
    /// Load runtime config from a TOML file path.
    pub fn from_toml_file(path: &std::path::Path) -> Result<Self, anyhow::Error> {
        let text = std::fs::read_to_string(path)?;
        let mut config: Self = toml::from_str(&text)?;
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Load from TOML string (used by tests).
    pub fn from_toml_str(text: &str) -> Result<Self, anyhow::Error> {
        let mut config: Self = toml::from_str(text)?;
        config.apply_env_overrides();
        config.validate()?;
        Ok(config)
    }

    /// Environment overrides for secret-adjacent runtime fields. VAPID keys may
    /// come from env (`PPB_VAPID_PRIVATE_KEY_PEM` / `PPB_VAPID_SUBJECT`) instead
    /// of the TOML `[notifications]` keys; env wins when set. (P-86 naming: the
    /// config keys `vapid_private_key_pem` / `vapid_subject` are canonical.)
    fn apply_env_overrides(&mut self) {
        if let Ok(pem) = std::env::var("PPB_VAPID_PRIVATE_KEY_PEM") {
            if !pem.trim().is_empty() {
                self.notifications.vapid_private_key_pem = Some(pem);
            }
        }
        if let Ok(subject) = std::env::var("PPB_VAPID_SUBJECT") {
            if !subject.trim().is_empty() {
                self.notifications.vapid_subject = Some(subject);
            }
        }
    }

    /// Basic sanity validation.
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        if self.cors.credentials && self.cors.allowed_origins.iter().any(|o| o == "*") {
            anyhow::bail!("cors.credentials=true forbids '*' allowed_origins");
        }
        if self.pmp.auth_mode != "token" && self.pmp.auth_mode != "approve" {
            anyhow::bail!("pmp.auth_mode must be 'token' or 'approve'");
        }
        let samesite = self.session.cookie_samesite.to_ascii_lowercase();
        if !matches!(samesite.as_str(), "lax" | "strict" | "none") {
            anyhow::bail!("session.cookie_samesite must be lax|strict|none");
        }
        if self.legal.public_auth_enabled {
            for (name, value) in [
                ("legal.terms_version", self.legal.terms_version.trim()),
                ("legal.privacy_version", self.legal.privacy_version.trim()),
                ("legal.terms_url", self.legal.terms_url.trim()),
                ("legal.privacy_url", self.legal.privacy_url.trim()),
            ] {
                if value.is_empty() {
                    anyhow::bail!("{name} is required when legal.public_auth_enabled=true");
                }
            }
            for (name, value) in [
                ("legal.terms_url", self.legal.terms_url.trim()),
                ("legal.privacy_url", self.legal.privacy_url.trim()),
            ] {
                let safe_relative = value.starts_with('/') && !value.starts_with("//") && !value.contains('\\');
                let safe_absolute = value.starts_with("https://")
                    || value.starts_with("http://localhost")
                    || value.starts_with("http://127.0.0.1");
                if !safe_relative && !safe_absolute {
                    anyhow::bail!("{name} must be HTTPS, localhost HTTP, or a safe relative path");
                }
            }
        }
        Ok(())
    }

    /// Merge runtime overrides (JSONB, from DB) on top of the boot-time config.
    ///
    /// Only keys that already exist in the config are overridden; unknown keys are
    /// ignored (validated against the schema by re-deserialization).
    pub fn apply_overrides(&self, overrides: &serde_json::Value) -> Result<Self, anyhow::Error> {
        let mut cfg = serde_json::to_value(self)?;
        if let (Some(base), Some(over)) = (cfg.as_object_mut(), overrides.as_object()) {
            merge_objects(base, over);
        }
        let merged: Self = serde_json::from_value(cfg)?;
        merged.validate()?;
        Ok(merged)
    }
}

fn merge_objects(base: &mut serde_json::Map<String, serde_json::Value>, over: &serde_json::Map<String, serde_json::Value>) {
    for (k, v) in over {
        match (base.get_mut(k), v) {
            (Some(serde_json::Value::Object(b)), serde_json::Value::Object(o)) => {
                merge_objects(b, o);
            }
            (Some(b), o) => {
                *b = o.clone();
            }
            (None, _) => {} // unknown key: ignore
        }
    }
}

/// Default runtime config source resolution:
/// 1. PPB_RUNTIME_CONFIG env
/// 2. ./config/ppb.toml
/// 3. baked defaults
pub fn resolve_runtime_config() -> Result<RuntimeConfig, anyhow::Error> {
    let config = if let Ok(path) = std::env::var("PPB_RUNTIME_CONFIG") {
        RuntimeConfig::from_toml_file(std::path::Path::new(&path))?
    } else {
        let default_path = std::path::Path::new("config/ppb.toml");
        if default_path.exists() {
            RuntimeConfig::from_toml_file(default_path)?
        } else {
            let mut defaults = RuntimeConfig::default();
            defaults.apply_env_overrides();
            return Ok(defaults);
        }
    };
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_example_config() {
        let text = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/example.toml"),
        )
        .expect("example.toml present");
        let config = RuntimeConfig::from_toml_str(&text).expect("parses");
        assert_eq!(config.server.public_url, "https://api-phira.htadiy.com");
        assert_eq!(config.pmp.auth_mode, "approve");
        assert_eq!(config.pmp.capabilities.len(), 6);
    }

    #[test]
    fn rejects_star_origin_with_credentials() {
        let text = r#"
[server]
listen_addr = "0.0.0.0:8080"
[cors]
credentials = true
allowed_origins = ["*"]
"#;
        assert!(RuntimeConfig::from_toml_str(text).is_err());
    }

    #[test]
    fn defaults_are_valid() {
        let config = RuntimeConfig::default();
        config.validate().unwrap();
    }
}
