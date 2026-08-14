//! Encrypted-at-rest Phira refresh tokens.
//!
//! Access tokens are short-lived memory-only; refresh tokens are encrypted with
//! AES-256-GCM using a deployment secret key (PPB_PHIRA_CREDENTIAL_KEY). The
//! password is never stored. Blob layout: `nonce(12) || ciphertext`.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;

use crate::error::{ApiError, ErrorCode};

const NONCE_LEN: usize = 12;

/// AES-256-GCM cipher over Phira credentials.
#[derive(Clone)]
pub struct CredentialCipher {
    cipher: Aes256Gcm,
}

impl CredentialCipher {
    pub fn new(key: &[u8]) -> Result<Self, ApiError> {
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|_| ApiError::new(ErrorCode::InternalError, "invalid credential key length"))?;
        Ok(Self { cipher })
    }

    /// Encrypt plaintext to `nonce || ciphertext`.
    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, ApiError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = self
            .cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| ApiError::new(ErrorCode::InternalError, "credential encryption failed"))?;
        let mut blob = Vec::with_capacity(NONCE_LEN + ct.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);
        Ok(blob)
    }

    /// Decrypt `nonce || ciphertext`.
    pub fn decrypt(&self, blob: &[u8]) -> Result<Vec<u8>, ApiError> {
        if blob.len() < NONCE_LEN {
            return Err(ApiError::new(ErrorCode::InternalError, "credential blob too short"));
        }
        let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
        let nonce = Nonce::from_slice(nonce_bytes);
        self.cipher
            .decrypt(nonce, ct)
            .map_err(|_| ApiError::new(ErrorCode::InternalError, "credential decryption failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let cipher = CredentialCipher::new(&[7u8; 32]).unwrap();
        let plaintext = b"phira_refresh_token_secret";
        let blob = cipher.encrypt(plaintext).unwrap();
        // GCM appends a 16-byte authentication tag, so the blob is longer
        // than NONCE_LEN + plaintext length.
        assert!(blob.len() > NONCE_LEN + plaintext.len());
        assert_ne!(&blob[NONCE_LEN..], plaintext.as_slice());
        let decrypted = cipher.decrypt(&blob).unwrap();
        assert_eq!(decrypted, plaintext.as_slice());
    }

    #[test]
    fn wrong_key_fails() {
        let cipher = CredentialCipher::new(&[7u8; 32]).unwrap();
        let other = CredentialCipher::new(&[8u8; 32]).unwrap();
        let blob = cipher.encrypt(b"token").unwrap();
        assert!(other.decrypt(&blob).is_err());
    }

    #[test]
    fn short_blob_fails() {
        let cipher = CredentialCipher::new(&[7u8; 32]).unwrap();
        assert!(cipher.decrypt(&[1, 2, 3]).is_err());
    }
}
