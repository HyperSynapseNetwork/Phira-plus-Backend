//! Request-id plumbing. Uses tower-http `X-Request-Id` layers.

use axum::http::request::Parts;
use axum::http::HeaderName;
use tower_http::request_id::{MakeRequestUuid, RequestId};
use uuid::Uuid;

/// Standard request id header.
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// Compose the request-id tower layers (set + propagate). Apply via a ServiceBuilder.
pub fn layers() -> tower_http::request_id::SetRequestIdLayer<MakeRequestUuid> {
    SetRequestIdLayer::new(X_REQUEST_ID, MakeRequestUuid)
}

/// Read the request id from request parts (generates one if absent).
pub fn read_request_id(parts: &Parts) -> String {
    parts
        .extensions
        .get::<RequestId>()
        .map(|id| id.as_str().to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::request::Request;
    use tower::ServiceExt as _;
    use tower_http::request_id::PropagateRequestIdLayer;

    #[tokio::test]
    async fn sets_and_propagates_request_id() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(layers())
            .layer(PropagateRequestIdLayer::new(X_REQUEST_ID))
            .service_fn(|req: Request<Body>| async move {
                let id = read_request_id(req.parts());
                assert!(!id.is_empty());
                Ok::<_, std::convert::Infallible>(axum::response::Response::new(Body::empty()))
            });

        let res = svc
            .oneshot(Request::new(Body::empty()))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(res.headers().get(X_REQUEST_ID).is_some());
    }
}
