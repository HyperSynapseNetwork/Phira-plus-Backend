//! Typed REST extractors that map Axum rejections to the frozen ErrorEnvelope.
//!
//! Business handlers consume `ApiJson/ApiQuery/ApiPath`; the outer request
//! middleware remains a safety net for legacy routes and framework 404/405.

use axum::extract::{FromRequest, FromRequestParts, Json, Path, Query, Request};
use axum::http::{request::Parts, StatusCode};
use serde::de::DeserializeOwned;

use super::{ApiError, ErrorCode};

pub struct ApiJson<T>(pub T);
pub struct ApiQuery<T>(pub T);
pub struct ApiPath<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => {
                let code = match rejection.status() {
                    StatusCode::UNSUPPORTED_MEDIA_TYPE => ErrorCode::InvalidContentType,
                    StatusCode::PAYLOAD_TOO_LARGE => ErrorCode::RequestBodyTooLarge,
                    _ => ErrorCode::InvalidJson,
                };
                tracing::debug!(
                    error = %rejection,
                    error_code = code.as_str(),
                    "request JSON extraction rejected"
                );
                Err(ApiError::new(code, match code {
                    ErrorCode::InvalidContentType => "request content type must be application/json",
                    ErrorCode::RequestBodyTooLarge => "request body too large",
                    _ => "invalid json request",
                }))
            }
        }
    }
}

impl<S, T> FromRequestParts<S> for ApiQuery<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(Self(value)),
            Err(rejection) => {
                tracing::debug!(error = %rejection, "request query extraction rejected");
                Err(ApiError::new(ErrorCode::InvalidQuery, "invalid query parameters"))
            }
        }
    }
}

impl<S, T> FromRequestParts<S> for ApiPath<T>
where
    S: Send + Sync,
    T: DeserializeOwned + Send,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, state).await {
            Ok(Path(value)) => Ok(Self(value)),
            Err(rejection) => {
                tracing::debug!(error = %rejection, "request path extraction rejected");
                Err(ApiError::new(ErrorCode::InvalidPathParam, "invalid path parameter"))
            }
        }
    }
}
