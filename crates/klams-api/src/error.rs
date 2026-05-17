//! Public API error type and HTTP mapping.
//!
//! The wire-format `ApiError` itself lives in `klams_types::ApiError`
//! so the typed client can deserialize it. This module defines the
//! server-side `ApiError` enum and its `IntoResponse` impl.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use klams_types::ApiError as WireApiError;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("validation error on field `{field}`: {message}")]
    Validation { field: String, message: String },
    #[error("unauthorized")]
    Unauthorized,
    #[error("payload too large")]
    TooLarge,
    #[error("queue at capacity")]
    QueueFull { retry_after: u32 },
    #[error("not found: {resource}")]
    NotFound { resource: String },
    #[error("internal server error (request {request_id})")]
    Internal { request_id: String },
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Validation { .. } => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::QueueFull { .. } => StatusCode::SERVICE_UNAVAILABLE,
            ApiError::NotFound { .. } => StatusCode::NOT_FOUND,
            ApiError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn wire(&self) -> WireApiError {
        match self {
            ApiError::Validation { field, message } => WireApiError {
                code: "validation_error".into(),
                message: message.clone(),
                field: Some(field.clone()),
                request_id: None,
            },
            ApiError::Unauthorized => WireApiError {
                code: "unauthorized".into(),
                message: "missing or invalid bearer token".into(),
                field: None,
                request_id: None,
            },
            ApiError::TooLarge => WireApiError {
                code: "payload_too_large".into(),
                message: "request payload exceeds configured limit".into(),
                field: None,
                request_id: None,
            },
            ApiError::QueueFull { .. } => WireApiError {
                code: "queue_full".into(),
                message: "write queue at capacity; retry later".into(),
                field: None,
                request_id: None,
            },
            ApiError::NotFound { resource } => WireApiError {
                code: "not_found".into(),
                message: format!("no such {resource}"),
                field: None,
                request_id: None,
            },
            ApiError::Internal { request_id } => WireApiError {
                code: "internal_error".into(),
                message: "internal server error".into(),
                field: None,
                request_id: Some(request_id.clone()),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(self.wire());
        let mut response = (status, body).into_response();
        if let ApiError::QueueFull { retry_after } = self {
            if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, v);
            }
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    async fn body_json(resp: Response) -> (StatusCode, WireApiError, axum::http::HeaderMap) {
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
        let wire: WireApiError = serde_json::from_slice(&body).unwrap();
        (status, wire, headers)
    }

    #[tokio::test]
    async fn validation_error_maps_to_400() {
        let resp = ApiError::Validation {
            field: "type".into(),
            message: "unknown FactType".into(),
        }
        .into_response();
        let (status, wire, _) = body_json(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(wire.code, "validation_error");
        assert_eq!(wire.field.as_deref(), Some("type"));
    }

    #[tokio::test]
    async fn unauthorized_maps_to_401() {
        let resp = ApiError::Unauthorized.into_response();
        let (status, wire, _) = body_json(resp).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(wire.code, "unauthorized");
    }

    #[tokio::test]
    async fn too_large_maps_to_413() {
        let resp = ApiError::TooLarge.into_response();
        let (status, wire, _) = body_json(resp).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(wire.code, "payload_too_large");
    }

    #[tokio::test]
    async fn queue_full_maps_to_503_with_retry_after() {
        let resp = ApiError::QueueFull { retry_after: 5 }.into_response();
        let (status, wire, headers) = body_json(resp).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(wire.code, "queue_full");
        assert_eq!(headers.get(header::RETRY_AFTER).unwrap(), "5");
    }

    #[tokio::test]
    async fn internal_maps_to_500_with_request_id() {
        let resp = ApiError::Internal {
            request_id: "req-abc".into(),
        }
        .into_response();
        let (status, wire, _) = body_json(resp).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(wire.code, "internal_error");
        assert_eq!(wire.request_id.as_deref(), Some("req-abc"));
    }
}
