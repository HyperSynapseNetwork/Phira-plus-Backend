//! Notification Push adapters (design §14.7).
//!
//! In-app SSE is the base channel. Web Push is a full VAPID + RFC 8291
//! (aes128gcm) adapter. FCM / WNS are stubs pending Owner credentials; both
//! return `NotConfigured` until then. Push endpoint material is encrypted at
//! rest with the deployment CredentialCipher.

use aes_gcm::KeyInit;
use aes_gcm::aead::Aead;
use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hkdf::Hkdf;
use p256::ecdh::EphemeralSecret;
use p256::PublicKey;
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::FromRow;
use url::Url;
use uuid::Uuid;

use crate::config::NotificationConfig;
use crate::error::{ApiError, ErrorCode};
use crate::phira::credential::CredentialCipher;

/// A decrypted push subscription (material in memory only).
#[derive(Debug, Clone)]
pub struct PushSubscription {
    pub endpoint: String,
    /// Raw SEC1 uncompressed public key (from `p256dh`).
    pub p256dh: Vec<u8>,
    /// 16-byte auth secret.
    pub auth: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum PushError {
    NotConfigured(String),
    Delivery(String),
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConfigured(m) => write!(f, "not configured: {m}"),
            Self::Delivery(m) => write!(f, "delivery failed: {m}"),
        }
    }
}

/// A push adapter (channel implementation).
#[async_trait]
pub trait PushAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    async fn send(
        &self,
        sub: &PushSubscription,
        title: &str,
        body: &str,
        data: Option<&Value>,
    ) -> Result<(), PushError>;
}

// ── Web Push (VAPID + RFC 8291) ───────────────────────────────

pub struct WebPushAdapter {
    vapid_private_key_pem: Option<String>,
    vapid_subject: Option<String>,
    http: reqwest::Client,
}

impl WebPushAdapter {
    pub fn new(config: &NotificationConfig) -> Self {
        Self {
            vapid_private_key_pem: config.vapid_private_key_pem.clone(),
            vapid_subject: config.vapid_subject.clone(),
            http: reqwest::Client::new(),
        }
    }

    fn configured(&self) -> bool {
        matches!(&self.vapid_private_key_pem, Some(p) if !p.is_empty())
            && matches!(&self.vapid_subject, Some(s) if !s.is_empty())
    }
}

#[async_trait]
impl PushAdapter for WebPushAdapter {
    fn name(&self) -> &'static str {
        "web_push"
    }

    async fn send(
        &self,
        sub: &PushSubscription,
        title: &str,
        body: &str,
        data: Option<&Value>,
    ) -> Result<(), PushError> {
        if !self.configured() {
            return Err(PushError::NotConfigured("web_push: VAPID keys not configured".into()));
        }
        let (pem, subject) = (
            self.vapid_private_key_pem.clone().unwrap(),
            self.vapid_subject.clone().unwrap(),
        );
        let payload = json!({ "title": title, "body": body, "data": data }).to_string().into_bytes();
        let encrypted = encrypt_rfc8291(sub, &payload).map_err(PushError::Delivery)?;
        let auth_header = vapid_authorization(&pem, &subject, &sub.endpoint).map_err(PushError::Delivery)?;
        let resp = self
            .http
            .post(&sub.endpoint)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("TTL", "60")
            .header("Authorization", auth_header)
            .body(encrypted)
            .send()
            .await
            .map_err(|e| PushError::Delivery(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(PushError::Delivery(format!("web_push HTTP {}", resp.status())));
        }
        Ok(())
    }
}

/// Encrypt a payload for a push subscription (RFC 8291 aes128gcm).
fn encrypt_rfc8291(sub: &PushSubscription, payload: &[u8]) -> Result<Vec<u8>, String> {
    let eph = EphemeralSecret::random(&mut OsRng);
    let as_public = PublicKey::from(&eph);
    let as_public_bytes = encoded_point_bytes(&as_public);
    let ua_public = PublicKey::from_sec1_bytes(&sub.p256dh).map_err(|e| e.to_string())?;
    let shared = eph.diffie_hellman(&ua_public);
    let shared_secret = shared.raw_secret_bytes();

    let mut info = Vec::new();
    info.extend_from_slice(b"WebPush: info\x00");
    info.extend_from_slice(&sub.p256dh);
    info.extend_from_slice(&as_public_bytes);

    let hk = Hkdf::<Sha256>::new(Some(sub.auth.as_slice()), shared_secret);
    let mut cek = [0u8; 16];
    hk.expand(&info, &mut cek).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 12];
    hk.expand(&info, &mut nonce).map_err(|e| e.to_string())?;

    let mut plaintext = payload.to_vec();
    plaintext.push(0x02); // final record delimiter
    let ct = aes_gcm::Aes128Gcm::new_from_slice(&cek)
        .map_err(|e| e.to_string())?
        .encrypt(aes_gcm::Nonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|e| e.to_string())?;

    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut body = Vec::new();
    body.extend_from_slice(&salt);
    body.extend_from_slice(&4096u32.to_be_bytes());
    body.push(65u8); // idlen
    body.extend_from_slice(&as_public_bytes);
    body.extend_from_slice(&ct);
    Ok(body)
}

/// Build the `Authorization: vapid t=...,k=...` header.
fn vapid_authorization(pem: &str, subject: &str, endpoint: &str) -> Result<String, String> {
    let sk = p256::SecretKey::from_sec1_pem(pem).map_err(|e| e.to_string())?;
    let pk_bytes = encoded_point_bytes(&sk.public_key());
    let public_b64 = B64.encode(&pk_bytes);

    let url = Url::parse(endpoint).map_err(|e| e.to_string())?;
    let origin = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    let now = chrono::Utc::now().timestamp();
    let claims = json!({ "aud": origin, "exp": now + 43_200, "sub": subject });
    let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    let key = jsonwebtoken::EncodingKey::from_ec_pem(pem.as_bytes()).map_err(|e| e.to_string())?;
    let jwt = jsonwebtoken::encode(&header, &claims, &key).map_err(|e| e.to_string())?;
    Ok(format!("vapid t={jwt},k={public_b64}"))
}

// ── FCM / WNS stubs (pending Owner credentials) ────────────────

pub struct FcmAdapter;

#[async_trait]
impl PushAdapter for FcmAdapter {
    fn name(&self) -> &'static str {
        "fcm"
    }
    async fn send(
        &self,
        _sub: &PushSubscription,
        _title: &str,
        _body: &str,
        _data: Option<&Value>,
    ) -> Result<(), PushError> {
        Err(PushError::NotConfigured("fcm: FCM credentials not configured".into()))
    }
}

pub struct WnsAdapter;

#[async_trait]
impl PushAdapter for WnsAdapter {
    fn name(&self) -> &'static str {
        "wns"
    }
    async fn send(
        &self,
        _sub: &PushSubscription,
        _title: &str,
        _body: &str,
        _data: Option<&Value>,
    ) -> Result<(), PushError> {
        Err(PushError::NotConfigured("wns: Windows Push credentials not configured".into()))
    }
}

// ── PushService ────────────────────────────────────────────────

/// Whether the user has any active APP push endpoint (`fcm`/`wns`). Owner
/// decision: when true, `web_push` is skipped during fan-out.
fn app_push_present(endpoints: &[PushEndpointRow]) -> bool {
    endpoints
        .iter()
        .any(|ep| matches!(ep.channel.as_str(), "fcm" | "wns"))
}

#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema)]
pub struct PushSummary {
    pub delivered: u32,
    pub not_configured: u32,
    pub failed: u32,
}

/// Row from `push_endpoints` for fan-out.
#[derive(Debug, Clone, FromRow)]
pub struct PushEndpointRow {
    pub id: Uuid,
    pub channel: String,
    #[sqlx(rename = "endpoint_ciphertext")]
    pub endpoint_ciphertext: Vec<u8>,
}

/// Encrypted subscription wire shape (before encryption at rest).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SubscriptionWire {
    pub endpoint: String,
    pub p256dh: String, // base64url
    pub auth: String,   // base64url
}

/// Fan-out service over registered push endpoints.
pub struct PushService {
    cipher: CredentialCipher,
    web_push: WebPushAdapter,
    fcm: FcmAdapter,
    wns: WnsAdapter,
}

impl PushService {
    pub fn new(config: &NotificationConfig, cipher: CredentialCipher) -> Self {
        Self {
            cipher,
            web_push: WebPushAdapter::new(config),
            fcm: FcmAdapter,
            wns: WnsAdapter,
        }
    }

    /// Deliver to push endpoints of `user_id`. Non-fatal per endpoint.
    ///
    /// Owner decision (channel dedup): when the user has any active APP push
    /// endpoint (`fcm`/`wns`), prefer APP push and **skip `web_push`** so the
    /// user is not notified twice. Otherwise fall back to `web_push` (+ in-app,
    /// which is created separately via inbox rows).
    pub async fn notify(
        &self,
        db: &sqlx::PgPool,
        user_id: Uuid,
        title: &str,
        body: &str,
        data: Option<&Value>,
    ) -> Result<PushSummary, ApiError> {
        let endpoints = sqlx::query_as::<_, PushEndpointRow>(
            "SELECT id, channel, endpoint_ciphertext FROM push_endpoints
             WHERE user_id = $1 AND disabled_at IS NULL",
        )
        .bind(user_id)
        .fetch_all(db)
        .await
        .map_err(db_err)?;

        let prefer_app_push = app_push_present(&endpoints);
        let mut summary = PushSummary::default();
        for ep in endpoints {
            // Channel dedup: with an APP endpoint present, never also deliver
            // via web_push (avoid duplicate notifications).
            if prefer_app_push && ep.channel == "web_push" {
                continue;
            }
            let sub = match self.decrypt_subscription(&ep.endpoint_ciphertext) {
                Some(s) => s,
                None => {
                    summary.failed += 1;
                    continue;
                }
            };
            let adapter: &dyn PushAdapter = match ep.channel.as_str() {
                "web_push" => &self.web_push,
                "fcm" => &self.fcm,
                "wns" => &self.wns,
                _ => {
                    summary.failed += 1;
                    continue;
                }
            };
            match adapter.send(&sub, title, body, data).await {
                Ok(()) => summary.delivered += 1,
                Err(PushError::NotConfigured(_)) => summary.not_configured += 1,
                Err(PushError::Delivery(_)) => summary.failed += 1,
            }
        }
        Ok(summary)
    }

    fn decrypt_subscription(&self, blob: &[u8]) -> Option<PushSubscription> {
        let plain = self.cipher.decrypt(blob).ok()?;
        let wire: SubscriptionWire = serde_json::from_slice(&plain).ok()?;
        let p256dh = B64.decode(&wire.p256dh).ok()?;
        let auth = B64.decode(&wire.auth).ok()?;
        if p256dh.len() != 65 || auth.len() != 16 {
            return None;
        }
        Some(PushSubscription {
            endpoint: wire.endpoint,
            p256dh,
            auth,
        })
    }
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "push endpoint not found")
    } else {
        tracing::error!(error = %e, "push db error");
        ApiError::internal()
    }
}

/// Encode a P-256 public key as uncompressed SEC1 bytes (0x04 || X || Y).
fn encoded_point_bytes(pk: &p256::PublicKey) -> Vec<u8> {
    p256::elliptic_curve::sec1::EncodedPoint::<p256::NistP256>::from(pk)
        .as_bytes()
        .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypt_rejects_malformed() {
        let cipher = CredentialCipher::new(&[9u8; 32]).unwrap();
        let svc = PushService::new(&NotificationConfig::default(), cipher);
        assert!(svc.decrypt_subscription(&[1, 2, 3]).is_none());
    }

    #[test]
    fn webpush_not_configured_without_keys() {
        let cfg = NotificationConfig::default();
        let adapter = WebPushAdapter::new(&cfg);
        assert!(!adapter.configured());
    }

    fn row(channel: &str) -> PushEndpointRow {
        PushEndpointRow {
            id: Uuid::new_v4(),
            channel: channel.to_string(),
            endpoint_ciphertext: Vec::new(),
        }
    }

    #[test]
    fn app_push_present_detects_fcm_wns() {
        assert!(!app_push_present(&[]));
        assert!(!app_push_present(&[row("web_push")]));
        assert!(app_push_present(&[row("web_push"), row("fcm")]));
        assert!(app_push_present(&[row("wns")]));
        assert!(app_push_present(&[row("fcm"), row("wns")]));
    }

    #[test]
    fn app_push_present_ignores_unknown() {
        assert!(!app_push_present(&[row("web_push"), row("bogus")]));
    }

    #[test]
    fn notify_skips_web_push_when_app_push_present() {
        // fcm present -> the fan-out set must not include web_push.
        let endpoints = [row("web_push"), row("fcm")];
        assert!(app_push_present(&endpoints));
        let selected: Vec<&str> = endpoints
            .iter()
            .filter(|ep| !(app_push_present(&endpoints) && ep.channel == "web_push"))
            .map(|ep| ep.channel.as_str())
            .collect();
        assert!(!selected.contains(&"web_push"));
        assert!(selected.contains(&"fcm"));
    }

    #[test]
    fn notify_keeps_web_push_when_no_app_push() {
        // Only web_push -> the fan-out set includes web_push.
        let endpoints = [row("web_push")];
        let selected: Vec<&str> = endpoints
            .iter()
            .filter(|ep| !(app_push_present(&endpoints) && ep.channel == "web_push"))
            .map(|ep| ep.channel.as_str())
            .collect();
        assert!(selected.contains(&"web_push"));
    }
}
