//! Deployment / secret configuration.
//!
//! Secrets only come from the environment or a secret file. They are never
//! Serialize, never logged, never returned by Panel APIs. This struct is
//! intentionally NOT `Serialize`.

use std::env;

/// All deployment secrets loaded at boot.
#[derive(Debug, Clone)]
pub struct Secrets {
    pub database_url: Option<String>,
    pub jwt_secret: String,
    pub phira_credential_key: Vec<u8>,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    pub pmp_openuds_token: Option<String>,
}

fn env_or(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

impl Secrets {
    /// Load secrets from the environment.
    ///
    /// - `PPB_DATABASE_URL`
    /// - `PPB_JWT_SECRET` (required; base64, >= 32 bytes recommended)
    /// - `PPB_PHIRA_CREDENTIAL_KEY` (required; base64 of 32 bytes for AES-256-GCM)
    /// - `PPB_GITHUB_CLIENT_ID` / `PPB_GITHUB_CLIENT_SECRET`
    /// - `PPB_PMP_OPENUDS_TOKEN`
    pub fn from_env() -> Result<Self, anyhow::Error> {
        let jwt_secret = env_or("PPB_JWT_SECRET")
            .ok_or_else(|| anyhow::anyhow!("PPB_JWT_SECRET is required"))?;

        let credential_b64 = env_or("PPB_PHIRA_CREDENTIAL_KEY")
            .ok_or_else(|| anyhow::anyhow!("PPB_PHIRA_CREDENTIAL_KEY is required"))?;
        let credential_key = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            credential_b64,
        )
        .map_err(|e| anyhow::anyhow!("PPB_PHIRA_CREDENTIAL_KEY not valid base64: {e}"))?;
        if credential_key.len() != 32 {
            anyhow::bail!("PPB_PHIRA_CREDENTIAL_KEY must decode to 32 bytes (AES-256)");
        }

        Ok(Self {
            database_url: env_or("PPB_DATABASE_URL"),
            jwt_secret,
            phira_credential_key: credential_key,
            github_client_id: env_or("PPB_GITHUB_CLIENT_ID"),
            github_client_secret: env_or("PPB_GITHUB_CLIENT_SECRET"),
            pmp_openuds_token: env_or("PPB_PMP_OPENUDS_TOKEN"),
        })
    }

    /// Whether GitHub OAuth is configured.
    pub fn github_configured(&self) -> bool {
        self.github_client_id.is_some() && self.github_client_secret.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_key_length_validated() {
        // 32 zero bytes base64.
        let key = vec![0u8; 32];
        let b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            key,
        );
        assert_eq!(base64::Engine::decode::<Vec<u8>>(
            &base64::engine::general_purpose::STANDARD,
            b64,
        )
        .unwrap()
        .len(), 32);
    }
}
