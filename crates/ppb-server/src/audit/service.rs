//! Audit service — convenience wrappers.

use super::model::NewAuditEvent;
use super::repo;
use crate::auth::types::AuthPrincipal;
use crate::error::ApiError;

/// Record a redacted audit event for a principal.
#[allow(clippy::too_many_arguments)]
pub async fn record_principal(
    db: &sqlx::PgPool,
    auth: &AuthPrincipal,
    action: &str,
    resource_type: &str,
    resource_id: &str,
    parameters_redacted: serde_json::Value,
    result: &str,
    error_code: &str,
    command_id: &str,
    ip: &str,
    user_agent: &str,
) -> Result<(), ApiError> {
    let event = NewAuditEvent {
        principal_type: auth.principal_type.to_string(),
        actor_user_id: if auth.is_root() { None } else { Some(auth.sub) },
        actor_session_id: Some(auth.sid),
        action: action.to_string(),
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
        parameters_redacted,
        result: result.to_string(),
        error_code: error_code.to_string(),
        request_id: auth.request_id.clone(),
        command_id: command_id.to_string(),
        ip: ip.to_string(),
        user_agent: user_agent.to_string(),
    };
    repo::record(db, &event).await
}
