//! Bearer-token auth middleware.
//!
//! Uses constant-time comparison (`subtle`) to resist timing oracles.
//! Public paths (e.g. `/healthz`, `/metrics`) should be mounted
//! outside the protected router.

use crate::ApiError;
use axum::{
    body::Body,
    extract::State,
    http::{header::AUTHORIZATION, Request},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub struct AuthState {
    expected: Arc<Vec<u8>>,
}

impl AuthState {
    pub fn new(bearer_token: impl Into<String>) -> Self {
        Self {
            expected: Arc::new(bearer_token.into().into_bytes()),
        }
    }
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("token_len", &self.expected.len())
            .finish()
    }
}

/// Axum middleware: requires `Authorization: Bearer <token>` matching
/// the configured token via constant-time comparison.
pub async fn require_bearer(
    State(state): State<AuthState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?
        .trim();
    let provided = token.as_bytes();
    if provided.len() != state.expected.len() {
        return Err(ApiError::Unauthorized);
    }
    if provided.ct_eq(&state.expected).into() {
        Ok(next.run(req).await)
    } else {
        Err(ApiError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                AuthState::new("super-secret"),
                require_bearer,
            ))
            .route("/healthz", get(|| async { "ok" }))
    }

    #[tokio::test]
    async fn missing_header_is_unauthorized() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_token_is_unauthorized() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AUTHORIZATION, "Bearer wrong-token-x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_token_passes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AUTHORIZATION, "Bearer super-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn healthz_is_public() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
