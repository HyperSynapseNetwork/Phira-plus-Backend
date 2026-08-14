//! Unified REST error contract v1.1.
//!
//! Every non-2xx REST response is normalized to:
//! ```json
//! {"error":{"code":"ROOM_NOT_FOUND","message":"room not found","request_id":"...","details":{"params":{}}}}
//! ```
//!
//! `code` is the stable UI/business semantic. `message` is debug/legacy
//! fallback only and MUST NOT be used as the primary frontend string.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

pub mod extractors;
pub mod pagination;

/// Canonical machine-readable REST error registry.
///
/// Keep variants stable. New domain semantics get a new code; frontends consume
/// the generated OpenAPI union rather than maintaining a handwritten list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    // Common / framework
    InvalidJson,
    InvalidQuery,
    InvalidPathParam,
    MethodNotAllowed,
    RequestBodyTooLarge,
    InvalidContentType,
    ValidationFailed,
    RateLimited,
    AuthRequired,
    SessionExpired,
    PermissionDenied,
    CsrfInvalid,
    ResourceNotFound,
    ResourceConflict,
    CapabilityNotSupported,
    InternalError,
    LongJobAccepted,
    LongRunningActionRequiresJob,

    // Auth / identity
    PhiraAuthFailed,
    PhiraReauthRequired,
    PhiraApiUnavailable,
    GithubOauthNotConfigured,
    GithubIdentityNotBound,
    GithubOauthFailed,
    GithubApiUnavailable,
    GithubOauthStateInvalid,
    AuthLegalConsentRequired,
    AuthLegalDocumentsUnavailable,
    RootPasswordInvalid,
    RootPasswordChangeRequired,
    CurrentSessionRevokeForbidden,

    // PMP / command
    PmpUnavailable,
    PmpCommandFailed,
    PmpCommandTimeout,
    PmpCapabilityMissing,
    PmpConfigNotAvailable,
    PmpInvalidResponse,

    // Rooms / chat
    RoomNotFound,
    RoomIdRequired,
    RoomHostRequired,
    RoomUserNotPresent,
    RoomBatchTargetRequired,
    RoomBatchActionUnsupported,
    RoomMoveTargetRequired,
    RoomChatEmpty,
    RoomChatTooLong,

    // Social
    UserNotFound,
    AlreadyFriends,
    FriendRequestAlreadySent,
    FriendRequestNotFound,
    FriendRelationRequired,
    UserBlocked,

    // Replay
    ReplayNotFound,
    ReplayAccessDenied,
    ReplayVisibilityInvalid,
    ReplayPlayerRequired,
    ReplayShareInvalid,
    ReplayShareExpired,
    ReplayShareRevoked,

    // Notification
    NotificationNotFound,
    NotificationActionNotAvailable,
    NotificationActionTargetInvalid,
    NotificationInputNotAllowed,
    NotificationInputEmpty,
    NotificationInputTooLong,

    // Config / jobs / redemption
    ConfigValidationFailed,
    ConfigSnapshotNotFound,
    ConfigScopeInvalid,
    JobNotFound,
    JobTypeUnknown,
    JobAlreadyRunning,
    JobNotRetryable,
    JobNotCancellable,
    RedemptionCodeNotFound,
    RedemptionCodeAlreadyUsed,
    RedemptionCodeRevoked,
    RedemptionCodeExpired,
    RedemptionCodeLimitReached,
    RedemptionActionUnsupported,

    // Admin / group
    GroupNotFound,
    GroupNameRequired,
    UserActionTargetInvalid,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        use ErrorCode::*;
        match self {
            InvalidJson => "INVALID_JSON",
            InvalidQuery => "INVALID_QUERY",
            InvalidPathParam => "INVALID_PATH_PARAM",
            MethodNotAllowed => "METHOD_NOT_ALLOWED",
            RequestBodyTooLarge => "REQUEST_BODY_TOO_LARGE",
            InvalidContentType => "INVALID_CONTENT_TYPE",
            ValidationFailed => "VALIDATION_FAILED",
            RateLimited => "RATE_LIMITED",
            AuthRequired => "AUTH_REQUIRED",
            SessionExpired => "SESSION_EXPIRED",
            PermissionDenied => "PERMISSION_DENIED",
            CsrfInvalid => "CSRF_INVALID",
            ResourceNotFound => "RESOURCE_NOT_FOUND",
            ResourceConflict => "RESOURCE_CONFLICT",
            CapabilityNotSupported => "CAPABILITY_NOT_SUPPORTED",
            InternalError => "INTERNAL_ERROR",
            LongJobAccepted => "LONG_JOB_ACCEPTED",
            LongRunningActionRequiresJob => "LONG_RUNNING_ACTION_REQUIRES_JOB",
            PhiraAuthFailed => "PHIRA_AUTH_FAILED",
            PhiraReauthRequired => "PHIRA_REAUTH_REQUIRED",
            PhiraApiUnavailable => "PHIRA_API_UNAVAILABLE",
            GithubOauthNotConfigured => "GITHUB_OAUTH_NOT_CONFIGURED",
            GithubIdentityNotBound => "GITHUB_IDENTITY_NOT_BOUND",
            GithubOauthFailed => "GITHUB_OAUTH_FAILED",
            GithubApiUnavailable => "GITHUB_API_UNAVAILABLE",
            GithubOauthStateInvalid => "GITHUB_OAUTH_STATE_INVALID",
            AuthLegalConsentRequired => "AUTH_LEGAL_CONSENT_REQUIRED",
            AuthLegalDocumentsUnavailable => "AUTH_LEGAL_DOCUMENTS_UNAVAILABLE",
            RootPasswordInvalid => "ROOT_PASSWORD_INVALID",
            RootPasswordChangeRequired => "ROOT_PASSWORD_CHANGE_REQUIRED",
            CurrentSessionRevokeForbidden => "CURRENT_SESSION_REVOKE_FORBIDDEN",
            PmpUnavailable => "PMP_UNAVAILABLE",
            PmpCommandFailed => "PMP_COMMAND_FAILED",
            PmpCommandTimeout => "PMP_COMMAND_TIMEOUT",
            PmpCapabilityMissing => "PMP_CAPABILITY_MISSING",
            PmpConfigNotAvailable => "PMP_CONFIG_NOT_AVAILABLE",
            PmpInvalidResponse => "PMP_INVALID_RESPONSE",
            RoomNotFound => "ROOM_NOT_FOUND",
            RoomIdRequired => "ROOM_ID_REQUIRED",
            RoomHostRequired => "ROOM_HOST_REQUIRED",
            RoomUserNotPresent => "ROOM_USER_NOT_PRESENT",
            RoomBatchTargetRequired => "ROOM_BATCH_TARGET_REQUIRED",
            RoomBatchActionUnsupported => "ROOM_BATCH_ACTION_UNSUPPORTED",
            RoomMoveTargetRequired => "ROOM_MOVE_TARGET_REQUIRED",
            RoomChatEmpty => "ROOM_CHAT_EMPTY",
            RoomChatTooLong => "ROOM_CHAT_TOO_LONG",
            UserNotFound => "USER_NOT_FOUND",
            AlreadyFriends => "ALREADY_FRIENDS",
            FriendRequestAlreadySent => "FRIEND_REQUEST_ALREADY_SENT",
            FriendRequestNotFound => "FRIEND_REQUEST_NOT_FOUND",
            FriendRelationRequired => "FRIEND_RELATION_REQUIRED",
            UserBlocked => "USER_BLOCKED",
            ReplayNotFound => "REPLAY_NOT_FOUND",
            ReplayAccessDenied => "REPLAY_ACCESS_DENIED",
            ReplayVisibilityInvalid => "REPLAY_VISIBILITY_INVALID",
            ReplayPlayerRequired => "REPLAY_PLAYER_REQUIRED",
            ReplayShareInvalid => "REPLAY_SHARE_INVALID",
            ReplayShareExpired => "REPLAY_SHARE_EXPIRED",
            ReplayShareRevoked => "REPLAY_SHARE_REVOKED",
            NotificationNotFound => "NOTIFICATION_NOT_FOUND",
            NotificationActionNotAvailable => "NOTIFICATION_ACTION_NOT_AVAILABLE",
            NotificationActionTargetInvalid => "NOTIFICATION_ACTION_TARGET_INVALID",
            NotificationInputNotAllowed => "NOTIFICATION_INPUT_NOT_ALLOWED",
            NotificationInputEmpty => "NOTIFICATION_INPUT_EMPTY",
            NotificationInputTooLong => "NOTIFICATION_INPUT_TOO_LONG",
            ConfigValidationFailed => "CONFIG_VALIDATION_FAILED",
            ConfigSnapshotNotFound => "CONFIG_SNAPSHOT_NOT_FOUND",
            ConfigScopeInvalid => "CONFIG_SCOPE_INVALID",
            JobNotFound => "JOB_NOT_FOUND",
            JobTypeUnknown => "JOB_TYPE_UNKNOWN",
            JobAlreadyRunning => "JOB_ALREADY_RUNNING",
            JobNotRetryable => "JOB_NOT_RETRYABLE",
            JobNotCancellable => "JOB_NOT_CANCELLABLE",
            RedemptionCodeNotFound => "REDEMPTION_CODE_NOT_FOUND",
            RedemptionCodeAlreadyUsed => "REDEMPTION_CODE_ALREADY_USED",
            RedemptionCodeRevoked => "REDEMPTION_CODE_REVOKED",
            RedemptionCodeExpired => "REDEMPTION_CODE_EXPIRED",
            RedemptionCodeLimitReached => "REDEMPTION_CODE_LIMIT_REACHED",
            RedemptionActionUnsupported => "REDEMPTION_ACTION_UNSUPPORTED",
            GroupNotFound => "GROUP_NOT_FOUND",
            GroupNameRequired => "GROUP_NAME_REQUIRED",
            UserActionTargetInvalid => "USER_ACTION_TARGET_INVALID",
        }
    }

    /// Default HTTP status; business codes never encode transport by themselves.
    pub fn status(&self) -> StatusCode {
        use ErrorCode::*;
        match self {
            InvalidJson | InvalidQuery | InvalidPathParam | InvalidContentType => StatusCode::BAD_REQUEST,
            ValidationFailed
            | AuthLegalConsentRequired
            | RoomIdRequired
            | RoomBatchTargetRequired
            | RoomBatchActionUnsupported
            | RoomMoveTargetRequired
            | RoomChatEmpty
            | RoomChatTooLong
            | ReplayVisibilityInvalid
            | ReplayPlayerRequired
            | NotificationActionTargetInvalid
            | NotificationActionNotAvailable
            | NotificationInputNotAllowed
            | NotificationInputEmpty
            | NotificationInputTooLong
            | ConfigValidationFailed
            | ConfigScopeInvalid
            | GroupNameRequired
            | UserActionTargetInvalid => StatusCode::UNPROCESSABLE_ENTITY,
            RequestBodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AuthRequired | SessionExpired | PhiraAuthFailed | PhiraReauthRequired | RootPasswordInvalid | GithubOauthStateInvalid => StatusCode::UNAUTHORIZED,
            PermissionDenied
            | CsrfInvalid
            | FriendRelationRequired
            | UserBlocked
            | ReplayAccessDenied
            | CurrentSessionRevokeForbidden
            | RootPasswordChangeRequired => StatusCode::FORBIDDEN,
            ResourceNotFound
            | UserNotFound
            | FriendRequestNotFound
            | RoomNotFound
            | ReplayNotFound
            | NotificationNotFound
            | ConfigSnapshotNotFound
            | JobNotFound
            | JobTypeUnknown
            | RedemptionCodeNotFound
            | GroupNotFound => StatusCode::NOT_FOUND,
            ResourceConflict
            | AlreadyFriends
            | FriendRequestAlreadySent
            | JobAlreadyRunning
            | JobNotRetryable
            | JobNotCancellable
            | RedemptionCodeAlreadyUsed
            | RedemptionCodeRevoked
            | RedemptionCodeExpired
            | RedemptionCodeLimitReached
            | ReplayShareExpired
            | ReplayShareRevoked => StatusCode::CONFLICT,
            CapabilityNotSupported | PmpCapabilityMissing | PmpConfigNotAvailable | RedemptionActionUnsupported => StatusCode::NOT_IMPLEMENTED,
            PmpUnavailable | PmpCommandFailed | PmpCommandTimeout | PmpInvalidResponse | PhiraApiUnavailable | GithubApiUnavailable | GithubOauthFailed => StatusCode::BAD_GATEWAY,
            ReplayShareInvalid | GithubIdentityNotBound => StatusCode::UNAUTHORIZED,
            GithubOauthNotConfigured | AuthLegalDocumentsUnavailable => StatusCode::SERVICE_UNAVAILABLE,
            InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            LongJobAccepted => StatusCode::ACCEPTED,
            LongRunningActionRequiresJob => StatusCode::CONFLICT,
            RoomHostRequired | RoomUserNotPresent => StatusCode::CONFLICT,
        }
    }
}

/// Safe details body. Unknown backend structures are normalized into `params`
/// with scalar values only before a response leaves PPB.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ErrorDetails {
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
}

/// Serializable error body (`error` member).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    pub request_id: String,
    pub details: ErrorDetails,
}

/// Wrapper for the JSON `{ "error": ... }` envelope.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
        let raw_message = message.into();
        let message = if code == ErrorCode::InternalError {
            if raw_message != "internal server error" {
                tracing::error!(error = %raw_message, "internal error redacted from REST response");
            }
            "internal server error".to_string()
        } else {
            raw_message
        };
        Self {
            code,
            message,
            request_id: crate::middleware::request_id::current_request_id(),
            details: serde_json::json!({ "params": {} }),
            retry_after_secs: None,
        }
    }

    pub fn with_details(
        code: ErrorCode,
        message: impl Into<String>,
        details: serde_json::Value,
    ) -> Self {
        let mut error = Self::new(code, message);
        error.details = sanitize_details(details);
        error
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ValidationFailed, message)
    }

    pub fn not_found(resource: impl AsRef<str>) -> Self {
        let resource = resource.as_ref();
        Self::new(
            not_found_code(resource),
            format!("resource not found: {resource}"),
        )
        .with_param("resource", resource)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::ResourceConflict, message)
    }

    pub fn permission_denied() -> Self {
        Self::new(ErrorCode::PermissionDenied, "permission denied")
    }

    pub fn session(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::SessionExpired, msg)
    }

    pub fn internal() -> Self {
        Self::new(ErrorCode::InternalError, "internal server error")
    }

    pub fn with_param(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        let mut params = BTreeMap::new();
        if let Some(existing) = self.details.get("params").and_then(serde_json::Value::as_object) {
            for (key, value) in existing {
                if is_safe_scalar(value) {
                    params.insert(key.clone(), value.clone());
                }
            }
        }
        params.insert(key.into(), serde_json::Value::String(value.to_string()));
        self.details = serde_json::json!({ "params": params });
        self
    }

    /// Attach a request id explicitly. Normally request-context middleware does
    /// this automatically through `current_request_id()`.
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
        let request_id = if self.request_id.trim().is_empty() {
            crate::middleware::request_id::current_request_id()
        } else {
            self.request_id
        };
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: if self.code == ErrorCode::InternalError {
                    "internal server error".to_string()
                } else {
                    self.message
                },
                request_id: request_id.clone(),
                details: details_from_value(if self.code == ErrorCode::InternalError {
                    serde_json::json!({ "params": {} })
                } else {
                    self.details
                }),
            },
        };
        let mut resp = (status, Json(envelope)).into_response();
        if let Ok(v) = axum::http::HeaderValue::from_str(&request_id) {
            resp.headers_mut().insert(crate::middleware::request_id::X_REQUEST_ID, v);
        }
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
            _ => crate::middleware::request_id::current_request_id(),
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
                if matches!(&e, sqlx::Error::RowNotFound) {
                    ApiError::new(ErrorCode::ResourceNotFound, "not found").into_response()
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

fn details_from_value(value: serde_json::Value) -> ErrorDetails {
    let sanitized = sanitize_details(value);
    let params = sanitized
        .get("params")
        .and_then(serde_json::Value::as_object)
        .map(|object| object.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    ErrorDetails { params }
}

fn sanitize_details(value: serde_json::Value) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(object) = value.as_object() {
        if let Some(explicit) = object.get("params").and_then(serde_json::Value::as_object) {
            for (key, value) in explicit {
                if is_safe_scalar(value) {
                    params.insert(key.clone(), value.clone());
                }
            }
        } else {
            for (key, value) in object {
                if is_safe_scalar(value) {
                    params.insert(key.clone(), value.clone());
                }
            }
        }
    }
    serde_json::json!({ "params": params })
}

fn is_safe_scalar(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
}

/// Legacy resource helper used by older call sites. The caller provides a
/// resource name explicitly; arbitrary handler messages are never parsed here.
fn not_found_code(resource: &str) -> ErrorCode {
    let lower = resource.to_ascii_lowercase();
    if lower.contains("room") { ErrorCode::RoomNotFound }
    else if lower.contains("replay") || lower.contains("round") { ErrorCode::ReplayNotFound }
    else if lower.contains("notification") { ErrorCode::NotificationNotFound }
    else if lower.contains("friend request") { ErrorCode::FriendRequestNotFound }
    else if lower.contains("user") || lower.contains("profile") { ErrorCode::UserNotFound }
    else if lower.contains("snapshot") { ErrorCode::ConfigSnapshotNotFound }
    else if lower.contains("job") { ErrorCode::JobNotFound }
    else if lower.contains("redemption") || lower.contains("coupon") { ErrorCode::RedemptionCodeNotFound }
    else if lower.contains("group") { ErrorCode::GroupNotFound }
    else { ErrorCode::ResourceNotFound }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_envelope_shape() {
        let err = ApiError::new(ErrorCode::PhiraReauthRequired, "reauth required")
            .with_request_id("req-123");
        assert_eq!(err.code.status(), StatusCode::UNAUTHORIZED);
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: err.code,
                message: err.message.clone(),
                request_id: err.request_id.clone(),
                details: ErrorDetails::default(),
            },
        };
        let json = serde_json::to_value(&envelope).unwrap();
        assert_eq!(json["error"]["code"].as_str(), Some("PHIRA_REAUTH_REQUIRED"));
        assert_eq!(json["error"]["request_id"].as_str(), Some("req-123"));
        assert!(json["error"]["details"]["params"].is_object());
    }

    #[test]
    fn internal_error_redacts_details_and_message() {
        let err = ApiError::new(ErrorCode::InternalError, "postgres password=secret at /srv/db.rs:10");
        assert_eq!(err.code, ErrorCode::InternalError);
        assert_eq!(err.message, "internal server error");
    }

}
