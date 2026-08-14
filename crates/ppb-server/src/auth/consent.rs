//! Versioned account legal acceptance. This is distinct from analytics/cookie consent.
//! A row means the user has accepted a specific Terms/Privacy version pair at
//! least once; it is a version registry, not a per-login consent event log.

use uuid::Uuid;

use super::types::ClientType;
use crate::config::LegalConfig;
use crate::error::{ApiError, ErrorCode};

#[derive(Debug, Clone)]
pub struct LegalAcceptance {
    pub terms_version: String,
    pub privacy_version: String,
}

/// Resolve the currently approved legal versions. Public account auth remains
/// unavailable until deployment owners configure approved documents.
pub fn current_versions(legal: &LegalConfig) -> Result<LegalAcceptance, ApiError> {
    if !legal.public_auth_enabled
        || legal.terms_version.trim().is_empty()
        || legal.privacy_version.trim().is_empty()
        || legal.terms_url.trim().is_empty()
        || legal.privacy_url.trim().is_empty()
    {
        return Err(ApiError::new(
            ErrorCode::AuthLegalDocumentsUnavailable,
            "approved legal documents are not configured",
        ));
    }
    Ok(LegalAcceptance {
        terms_version: legal.terms_version.clone(),
        privacy_version: legal.privacy_version.clone(),
    })
}

pub fn validate_acceptance(
    legal: &LegalConfig,
    accepted: bool,
    terms_version: Option<&str>,
    privacy_version: Option<&str>,
) -> Result<LegalAcceptance, ApiError> {
    let current = current_versions(legal)?;
    let terms = terms_version.unwrap_or("");
    let privacy = privacy_version.unwrap_or("");
    if !accepted || terms != current.terms_version || privacy != current.privacy_version {
        return Err(ApiError::new(
            ErrorCode::AuthLegalConsentRequired,
            "current legal document versions must be explicitly accepted",
        )
        .with_param("terms_version", &current.terms_version)
        .with_param("privacy_version", &current.privacy_version));
    }
    Ok(current)
}

/// Whether this user has already accepted the currently configured version pair.
pub async fn has_current_acceptance(
    db: &sqlx::PgPool,
    user_id: Uuid,
    legal: &LegalConfig,
) -> Result<bool, ApiError> {
    let current = current_versions(legal)?;
    let found: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(
             SELECT 1 FROM account_legal_acceptances
             WHERE user_id=$1 AND terms_version=$2 AND privacy_version=$3
           )"#,
    )
    .bind(user_id)
    .bind(&current.terms_version)
    .bind(&current.privacy_version)
    .fetch_one(db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(found)
}

/// Determine whether a login must create a new acceptance row. Existing users
/// who already accepted the current version pair are not asked to re-accept on
/// every login. New users and users facing a version bump must explicitly
/// submit the current versions.
pub async fn acceptance_for_login(
    db: &sqlx::PgPool,
    existing_user_id: Option<Uuid>,
    legal: &LegalConfig,
    accepted: bool,
    terms_version: Option<&str>,
    privacy_version: Option<&str>,
) -> Result<Option<LegalAcceptance>, ApiError> {
    // Fail closed if approved documents are not configured, even when a user
    // accepted an older pair in the past.
    current_versions(legal)?;
    if let Some(user_id) = existing_user_id {
        if has_current_acceptance(db, user_id, legal).await? {
            return Ok(None);
        }
    }
    Ok(Some(validate_acceptance(
        legal,
        accepted,
        terms_version,
        privacy_version,
    )?))
}

pub async fn record_acceptance(
    db: &sqlx::PgPool,
    user_id: Uuid,
    client_type: ClientType,
    acceptance: &LegalAcceptance,
    source: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"INSERT INTO account_legal_acceptances
           (user_id, terms_version, privacy_version, client_type, source)
           VALUES ($1,$2,$3,$4,$5)
           ON CONFLICT (user_id, terms_version, privacy_version) DO NOTHING"#,
    )
    .bind(user_id)
    .bind(&acceptance.terms_version)
    .bind(&acceptance.privacy_version)
    .bind(client_type.to_string())
    .bind(source)
    .execute(db)
    .await
    .map_err(|_| ApiError::internal())?;
    Ok(())
}
