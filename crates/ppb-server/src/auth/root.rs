//! Root — emergency/local principal (design §6.8).
//!
//! Root is NOT in `users`; lives in `root_credentials`. First-boot random
//! password printed via CLI path; forced password change on first login.
//! `ppctl root reset-password` (out of repo scope) can reuse `set_password`.

use rand::Rng;
use sqlx::PgPool;

use crate::error::{ApiError, ErrorCode};

pub const BCRYPT_COST: u32 = 12;

#[derive(Debug, Clone)]
pub struct RootLoginOutcome {
    pub must_change_password: bool,
}

/// Root auth service (stateless; DB-backed).
#[derive(Debug, Clone, Default)]
pub struct RootAuthService {
    _private: (),
}

impl RootAuthService {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Ensure a root_credentials row exists. On first boot, generates a random
    /// password, stores its hash, and returns the plaintext so the CLI can
    /// print it exactly once. Returns `None` if credentials already exist.
    pub async fn bootstrap(db: &PgPool) -> Result<Option<String>, ApiError> {
        let exists: (bool,) = sqlx::query_as::<_, (bool,)>(
            "SELECT EXISTS(SELECT 1 FROM root_credentials WHERE id = 1)",
        )
        .fetch_one(db)
        .await
        .map_err(db_err)?;
        if exists.0 {
            return Ok(None);
        }
        let password = generate_random_password();
        let hash = bcrypt::hash(&password, BCRYPT_COST)
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("root hash: {e}")))?;
        sqlx::query(
            "INSERT INTO root_credentials (id, password_hash, must_change_password)
             VALUES (1, $1, TRUE)
             ON CONFLICT (id) DO UPDATE
                SET password_hash = EXCLUDED.password_hash,
                    must_change_password = EXCLUDED.must_change_password,
                    updated_at = now()",
        )
        .bind(&hash)
        .execute(db)
        .await
        .map_err(db_err)?;
        Ok(Some(password))
    }

    pub async fn verify(db: &PgPool, password: &str) -> Result<RootLoginOutcome, ApiError> {
        let row: Option<(String, bool)> = sqlx::query_as::<_, (String, bool)>(
            "SELECT password_hash, must_change_password FROM root_credentials WHERE id = 1",
        )
        .fetch_optional(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ApiError::new(ErrorCode::Auth, "root not initialized"))?;

        let (hash, must_change_password) = row;
        let ok = bcrypt::verify(password, &hash)
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("root verify: {e}")))?;
        if !ok {
            return Err(ApiError::new(ErrorCode::Auth, "invalid root password"));
        }
        Ok(RootLoginOutcome { must_change_password })
    }

    pub async fn must_change_password(db: &PgPool) -> Result<bool, ApiError> {
        let row: (bool,) = sqlx::query_as::<_, (bool,)>(
            "SELECT must_change_password FROM root_credentials WHERE id = 1",
        )
        .fetch_one(db)
        .await
        .map_err(db_err)?;
        Ok(row.0)
    }

    /// Set a new password and clear the must-change flag.
    pub async fn change_password(db: &PgPool, new_password: &str) -> Result<(), ApiError> {
        if new_password.len() < 12 {
            return Err(ApiError::validation("root password must be at least 12 chars"));
        }
        let hash = bcrypt::hash(new_password, BCRYPT_COST)
            .map_err(|e| ApiError::new(ErrorCode::Internal, format!("root hash: {e}")))?;
        sqlx::query(
            "UPDATE root_credentials
             SET password_hash = $1, must_change_password = FALSE, updated_at = now()
             WHERE id = 1",
        )
        .bind(&hash)
        .execute(db)
        .await
        .map_err(db_err)?;
        Ok(())
    }
}

/// Generate a 16-char password from an unambiguous alphabet.
pub fn generate_random_password() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%^&*";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}

fn db_err(e: sqlx::Error) -> ApiError {
    if matches!(&e, sqlx::Error::RowNotFound) {
        ApiError::new(ErrorCode::NotFound, "root credentials not found")
    } else {
        tracing::error!(error = %e, "root db error");
        ApiError::internal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_password_length_and_charset() {
        let pw = generate_random_password();
        assert_eq!(pw.len(), 16);
        assert!(pw.chars().all(|c| !c.is_whitespace()));
        // Two draws should differ (statistically certain).
        assert_ne!(pw, generate_random_password());
    }

    #[test]
    fn bcrypt_round_trip() {
        let hash = bcrypt::hash("correct horse battery staple", BCRYPT_COST).unwrap();
        assert!(bcrypt::verify("correct horse battery staple", &hash).unwrap());
        assert!(!bcrypt::verify("wrong", &hash).unwrap());
    }
}
