//! Public API error type and HTTP mapping.
//!
//! The wire-format `ApiError` itself lives in `klams_types::ApiError`
//! so the typed client can deserialize it. This module defines the
//! server-side `ApiError` enum and its `IntoResponse` impl.

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use klams_types::{ApiError as WireApiError, ErrorDetail};
use uuid::Uuid;

/// Stable wire-level `code` values. These appear in the `OpenAPI`
/// contract and clients match on them.
pub const VALIDATION_ERROR: &str = "validation_error";
pub const VERSION_CONFLICT: &str = "version_conflict";
pub const TRUST_REQUIRED: &str = "trust_required";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("validation error on field `{field}`: {message}")]
    Validation { field: String, message: String },
    #[error("validation error: {} field(s)", .details.len())]
    ValidationDetailed { details: Vec<ErrorDetail> },
    #[error("version conflict on fact {fact_id} (current_version={current_version})")]
    VersionConflict { fact_id: Uuid, current_version: i32 },
    #[error("trust required: {message}")]
    TrustRequired { message: String },
    #[error("unauthorized")]
    Unauthorized,
    #[error("payload too large")]
    TooLarge,
    #[error("queue at capacity")]
    QueueFull { retry_after: u32 },
    #[error("all retrieval sources are unavailable")]
    AllSourcesUnavailable { retry_after: u32 },
    #[error("not found: {resource}")]
    NotFound { resource: String },
    #[error("gone: {what}")]
    Gone { what: String },
    #[error("internal server error (request {request_id})")]
    Internal { request_id: String },
    #[error("not implemented: {what}")]
    NotImplemented { what: String },
}

impl ApiError {
    /// Build a `validation_error` response from a list of structured
    /// `ErrorDetail`s. Used by the per-type validator registry.
    #[must_use]
    pub fn validation_error(details: Vec<ErrorDetail>) -> Self {
        ApiError::ValidationDetailed { details }
    }

    /// Build a `version_conflict` response. The wire body will carry
    /// `current_version` so the caller can retry against the latest
    /// state.
    #[must_use]
    pub fn version_conflict(current_version: i32, fact_id: Uuid) -> Self {
        ApiError::VersionConflict {
            fact_id,
            current_version,
        }
    }

    /// Build a `trust_required` (HTTP 403) response — used by
    /// promote/discard endpoints that require `User` or `Controller`
    /// trust.
    #[must_use]
    pub fn trust_required(message: impl Into<String>) -> Self {
        ApiError::TrustRequired {
            message: message.into(),
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ApiError::Validation { .. } => StatusCode::BAD_REQUEST,
            ApiError::ValidationDetailed { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            ApiError::VersionConflict { .. } => StatusCode::CONFLICT,
            ApiError::TrustRequired { .. } => StatusCode::FORBIDDEN,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ApiError::QueueFull { .. } | ApiError::AllSourcesUnavailable { .. } => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            ApiError::NotFound { .. } => StatusCode::NOT_FOUND,
            ApiError::Gone { .. } => StatusCode::GONE,
            ApiError::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
        }
    }

    fn wire(&self) -> WireApiError {
        match self {
            ApiError::Validation { field, message } => WireApiError {
                code: VALIDATION_ERROR.into(),
                message: message.clone(),
                field: Some(field.clone()),
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::ValidationDetailed { details } => WireApiError {
                code: VALIDATION_ERROR.into(),
                message: "request failed validation".into(),
                field: details.first().map(|d| d.field.clone()),
                request_id: None,
                details: Some(details.clone()),
                current_version: None,
            },
            ApiError::VersionConflict {
                current_version, ..
            } => WireApiError {
                code: VERSION_CONFLICT.into(),
                message: "expected_version does not match the current stored version".into(),
                field: Some("expected_version".into()),
                request_id: None,
                details: None,
                current_version: Some(*current_version),
            },
            ApiError::TrustRequired { message } => WireApiError {
                code: TRUST_REQUIRED.into(),
                message: message.clone(),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::Unauthorized => WireApiError {
                code: "unauthorized".into(),
                message: "missing or invalid bearer token".into(),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::TooLarge => WireApiError {
                code: "payload_too_large".into(),
                message: "request payload exceeds configured limit".into(),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::QueueFull { .. } => WireApiError {
                code: "queue_full".into(),
                message: "write queue at capacity; retry later".into(),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::AllSourcesUnavailable { .. } => WireApiError {
                code: "all_sources_unavailable".into(),
                message: "all retrieval sources are unavailable; retry later".into(),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::NotFound { resource } => WireApiError {
                code: "not_found".into(),
                message: format!("no such {resource}"),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::Gone { what } => WireApiError {
                code: "gone".into(),
                message: what.clone(),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
            },
            ApiError::Internal { request_id } => WireApiError {
                code: "internal_error".into(),
                message: "internal server error".into(),
                field: None,
                request_id: Some(request_id.clone()),
                details: None,
                current_version: None,
            },
            ApiError::NotImplemented { what } => WireApiError {
                code: "not_implemented".into(),
                message: format!("{what} is not implemented yet"),
                field: None,
                request_id: None,
                details: None,
                current_version: None,
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
        if let ApiError::AllSourcesUnavailable { retry_after } = self {
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
