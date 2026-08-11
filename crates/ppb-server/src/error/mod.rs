//! Unified error contract.
//!
//! Every API error is serialized as:
//! ```json
//! {"error":{"code":"PHIRA_REAUTH_REQUIRED","message":"...","request_id":"...","details":{}}}
//! ```
//! Frontends localize on `code`. Codes are upper-snake (see docs/PHASE_A_PLAN.md P4).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

pub mod pagination;

/// Canonical error codes (upper-snake). Frontend localizes on these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// Generic request id plumbing failure (should never surface as a user error).
    RequestId,
    /// Invalid pagination parameters.
    Pagination,
    /// Validation failure.
    Validation,
    /// Rate limited (HTTP 429 + Retry-After).
    RateLimit,
    /// Authentication failure (bad credentials / token).
    Auth,
    /// Session missing / expired / revoked.
    Session,
    /// Insufficient permission.
    PermissionDenied,
    /// PMP OpenUDS unavailable.
    PmpUnavailable,
    /// PMP lacks a required capability.
    CapabilityNotSupported,
    /// Phira API unavailable.
    PhiraApiUnavailable,
    /// Phira credential needs re-authentication.
    PhiraReauthRequired,
    /// Long-running job accepted (HTTP 202).
    LongJobAccepted,
    /// Not found.
    NotFound,
    /// Conflict / duplicate.
    Conflict,
    /// Internal server error (details redacted).
    Internal,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RequestId => "REQUEST_ID",
            Self::Pagination => "PAGINATION",
            Self::Validation => "VALIDATION",
            Self::RateLimit => "RATE_LIMIT",
            Self::Auth => "AUTH",
            Self::Session => "SESSION",
            Self::PermissionDenied => "PERMISSION_DENIED",
            Self::PmpUnavailable => "PMP_UNAVAILABLE",
            Self::CapabilityNotSupported => "CAPABILITY_NOT_SUPPORTED",
            Self::PhiraApiUnavailable => "PHIRA_API_UNAVAILABLE",
            Self::PhiraReauthRequired => "PHIRA_REAUTH_REQUIRED",
            Self::LongJobAccepted => "LONG_JOB_ACCEPTED",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "CONFLICT",
            Self::Internal => "INTERNAL",
        }
    }

    /// Default HTTP status for this code.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::RequestId => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Pagination => StatusCode::BAD_REQUEST,
            Self::Validation => StatusCode::BAD_REQUEST,
            Self::RateLimit => StatusCode::TOO_MANY_REQUESTS,
            Self::Auth => StatusCode::UNAUTHORIZED,
            Self::Session => StatusCode::UNAUTHORIZED,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::PmpUnavailable => StatusCode::BAD_GATEWAY,
            Self::CapabilityNotSupported => StatusCode::NOT_IMPLEMENTED,
            Self::PhiraApiUnavailable => StatusCode::BAD_GATEWAY,
            Self::PhiraReauthRequired => StatusCode::UNAUTHORIZED,
            Self::LongJobAccepted => StatusCode::ACCEPTED,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Serializable error body (`error` member).
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub request_id: String,
    pub details: serde_json::Value,
}

/// Wrapper for the JSON `{ "error": ... }` envelope.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

/// An API error ready to be returned to a client.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
    pub details: serde_json::Value,
    /// When set (rate limit), adds `Retry-After` to the response.
    pub retry_after_secs: Option<u64>,
}

impl ApiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            request_id: String::new(),
            details: serde_json::Value::Null,
            retry_after_secs: None,
        }
    }

    pub fn with_details(
        code: ErrorCode,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            request_id: String::new(),
            details,
            retry_after_secs: None,
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Validation, message)
    }

    pub fn not_found(resource: impl AsRef<str>) -> Self {
        Self::new(
            ErrorCode::NotFound,
            format!("resource not found: {}", resource.as_ref()),
        )
    }

    pub fn permission_denied() -> Self {
        Self::new(ErrorCode::PermissionDenied, "permission denied")
    }

    pub fn session(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Session, msg)
    }

    pub fn internal() -> Self {
        Self::new(ErrorCode::Internal, "internal server error")
    }

    /// Attach a request id (used by middleware / handlers).
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = request_id.into();
        self
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.code.status();
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.code.as_str().to_string(),
                message: self.message,
                request_id: self.request_id,
                details: self.details,
            },
        };
        let mut resp = (status, Json(envelope)).into_response();
        if let Some(retry_after) = self.retry_after_secs {
            if let Ok(v) = axum::http::HeaderValue::from_str(&retry_after.to_string()) {
                resp.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
            }
        }
        resp
    }
}

/// Domain-level error combining internal and API errors.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("{0}")]
    Api(#[from] ApiError),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl AppError {
    pub fn internal(err: impl std::fmt::Display) -> Self {
        Self::Internal(anyhow::anyhow!("{err}"))
    }

    pub fn request_id(&self) -> String {
        match self {
            Self::Api(e) => e.request_id.clone(),
            _ => String::new(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::Internal(e) => {
                tracing::error!(error = %e, "internal error");
                ApiError::internal().into_response()
            }
            Self::Db(e) => {
                // RowNotFound -> NOT_FOUND; otherwise redacted internal.
                if matches!(&e, sqlx::Error::RowNotFound) {
                    ApiError::new(ErrorCode::NotFound, "not found").into_response()
                } else {
                    tracing::error!(error = %e, "database error");
                    ApiError::internal().into_response()
                }
            }
            Self::Io(e) => {
                tracing::error!(error = %e, "io error");
                ApiError::internal().into_response()
            }
        }
    }
}

/// Map an `anyhow`-style error into an internal `AppError` at a call site.
pub trait IntoAppError<T> {
    fn into_app_error(self) -> Result<T, AppError>;
}

impl<T, E: std::fmt::Display> IntoAppError<T> for Result<T, E> {
    fn into_app_error(self) -> Result<T, AppError> {
        self.map_err(|e| AppError::internal(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_envelope_shape() {
        let err = ApiError::new(ErrorCode::PhiraReauthRequired, "需要重新验证 Phira 身份")
            .with_request_id("req-123");
        assert_eq!(err.code.status(), StatusCode::UNAUTHORIZED);

        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: err.code.as_str().to_string(),
                message: err.message.clone(),
                request_id: err.request_id.clone(),
                details: serde_json::Value::Null,
            },
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["error"]["code"].as_str(), Some("PHIRA_REAUTH_REQUIRED"));
        assert_eq!(json["error"]["request_id"].as_str(), Some("req-123"));
        assert!(json["error"]["details"].is_null());
    }

    #[test]
    fn internal_error_redacts_details() {
        let err = ApiError::internal();
        assert_eq!(err.code, ErrorCode::Internal);
        assert_eq!(err.message, "internal server error");
    }
}
