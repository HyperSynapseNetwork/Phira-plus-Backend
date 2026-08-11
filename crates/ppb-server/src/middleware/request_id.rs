//! Request-id plumbing. Uses tower-http `X-Request-Id` layers.

use axum::http::{HeaderMap, HeaderName};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use uuid::Uuid;

/// Standard request id header.
pub const X_REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The layer that sets the request id (generating a UUID when absent).
pub fn layers() -> SetRequestIdLayer<MakeRequestUuid> {
    SetRequestIdLayer::new(X_REQUEST_ID, MakeRequestUuid)
}

/// The layer that copies the request id to the response `X-Request-Id` header.
pub fn propagate_layer() -> PropagateRequestIdLayer {
    PropagateRequestIdLayer::new(X_REQUEST_ID)
}

/// Read the request id from request headers (generates one if absent).
pub fn read_request_id(headers: &HeaderMap) -> String {
    headers
        .get(X_REQUEST_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    #[tokio::test]
    async fn sets_and_propagates_request_id() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceBuilder;

        let svc = ServiceBuilder::new()
            .layer(layers())
            .layer(propagate_layer())
            .service_fn(|req: Request<Body>| async move {
                let id = read_request_id(req.headers());
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
