//! Request context: request-id propagation + final REST error normalization.
//!
//! This layer is deliberately outermost. It provides one request id to
//! extractors/handlers and rewrites framework/catch-panic rejections into the
//! same ErrorEnvelope contract without buffering successful streaming bodies.

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt as _;
use uuid::Uuid;

use crate::error::{ApiError, ErrorCode, ErrorEnvelope};

/// Standard request id header.
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

tokio::task_local! {
    static CURRENT_REQUEST_ID: String;
}

/// Read an existing request id or generate one. Invalid/untrusted header values
/// are not reflected; only a short printable token is accepted.
pub fn read_request_id(headers: &HeaderMap) -> String {
    headers
        .get(&X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty() && v.len() <= 128 && v.bytes().all(|b| b.is_ascii_graphic()))
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// Current request id for ApiError constructors. Background code receives a
/// fresh id, while HTTP handlers inherit the request-context id.
pub fn current_request_id() -> String {
    CURRENT_REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| Uuid::new_v4().to_string())
}

/// Outermost middleware: inject one request id, then normalize every REST error
/// response including Axum extractor rejections, 404/405 and panic fallback.
pub async fn request_context(mut req: Request<Body>, next: Next) -> Response {
    let request_id = read_request_id(req.headers());
    if let Ok(value) = HeaderValue::from_str(&request_id) {
        req.headers_mut().insert(X_REQUEST_ID, value);
    }

    CURRENT_REQUEST_ID
        .scope(request_id.clone(), async move {
            let response = next.run(req).await;
            normalize_error_response(response, &request_id).await
        })
        .await
}

async fn normalize_error_response(response: Response, request_id: &str) -> Response {
    if !response.status().is_client_error() && !response.status().is_server_error() {
        return with_request_header(response, request_id);
    }

    let status = response.status();
    let (mut parts, body) = response.into_parts();
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            tracing::error!(request_id, %error, "failed to collect error response body");
            Bytes::new()
        }
    };

    let envelope = serde_json::from_slice::<ErrorEnvelope>(&bytes).ok();
    let body = if let Some(mut envelope) = envelope {
        // Body/header correlation is enforced here even for manually-built
        // envelopes. INTERNAL is redacted a second time at the boundary.
        envelope.error.request_id = request_id.to_string();
        if envelope.error.code == ErrorCode::InternalError {
            envelope.error.message = "internal server error".to_string();
            envelope.error.details = Default::default();
        }
        serde_json::to_vec(&envelope).unwrap_or_else(|_| fallback_internal(request_id))
    } else {
        let original = String::from_utf8_lossy(&bytes);
        let code = framework_error_code(status, &original);
        let message = framework_message(code);
        let error = ApiError::new(code, message).with_request_id(request_id);
        match error.into_response().into_body().collect().await {
            Ok(collected) => collected.to_bytes().to_vec(),
            Err(_) => fallback_internal(request_id),
        }
    };

    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json; charset=utf-8"));
    if let Ok(value) = HeaderValue::from_str(request_id) {
        parts.headers.insert(X_REQUEST_ID, value);
    }
    Response::from_parts(parts, Body::from(body))
}

fn framework_error_code(status: StatusCode, _body: &str) -> ErrorCode {
    // JSON/query/path semantics belong to typed ApiJson/ApiQuery/ApiPath
    // extractors. This outer layer classifies only protocol-level fallback
    // responses by status; it never parses framework English body text.
    match status {
        StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY => ErrorCode::ValidationFailed,
        StatusCode::NOT_FOUND => ErrorCode::ResourceNotFound,
        StatusCode::METHOD_NOT_ALLOWED => ErrorCode::MethodNotAllowed,
        StatusCode::PAYLOAD_TOO_LARGE => ErrorCode::RequestBodyTooLarge,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => ErrorCode::InvalidContentType,
        StatusCode::UNAUTHORIZED => ErrorCode::AuthRequired,
        StatusCode::FORBIDDEN => ErrorCode::PermissionDenied,
        StatusCode::TOO_MANY_REQUESTS => ErrorCode::RateLimited,
        status if status.is_server_error() => ErrorCode::InternalError,
        _ => ErrorCode::ValidationFailed,
    }
}

fn framework_message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::InvalidJson => "invalid json request",
        ErrorCode::InvalidQuery => "invalid query parameters",
        ErrorCode::InvalidPathParam => "invalid path parameter",
        ErrorCode::MethodNotAllowed => "method not allowed",
        ErrorCode::RequestBodyTooLarge => "request body too large",
        ErrorCode::InvalidContentType => "invalid content type",
        ErrorCode::ResourceNotFound => "route not found",
        ErrorCode::AuthRequired => "authentication required",
        ErrorCode::PermissionDenied => "permission denied",
        ErrorCode::RateLimited => "rate limited",
        ErrorCode::InternalError => "internal server error",
        _ => "request validation failed",
    }
}

fn fallback_internal(request_id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "error": {
            "code": "INTERNAL_ERROR",
            "message": "internal server error",
            "request_id": request_id,
            "details": { "params": {} }
        }
    }))
    .unwrap_or_else(|_| b"{\"error\":{\"code\":\"INTERNAL_ERROR\"}}".to_vec())
}

fn with_request_header(mut response: Response, request_id: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(X_REQUEST_ID, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_mapping_is_stable() {
        assert_eq!(framework_error_code(StatusCode::BAD_REQUEST, "framework text is not parsed"), ErrorCode::ValidationFailed);
        assert_eq!(framework_error_code(StatusCode::METHOD_NOT_ALLOWED, ""), ErrorCode::MethodNotAllowed);
        assert_eq!(framework_error_code(StatusCode::INTERNAL_SERVER_ERROR, "secret path /srv"), ErrorCode::InternalError);
    }
}
