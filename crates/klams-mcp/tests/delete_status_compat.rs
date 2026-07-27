//! Sprint 018 (WI #305) — session-termination DELETE must surface 204.
//!
//! rmcp 1.7's `StreamableHttpService::handle_delete` hardcodes
//! 202 Accepted, but the mcp python-sdk client treats only 200/204 as
//! successful termination and logs `Session termination failed: 202`
//! on every session close. klams's `/mcp` mount rewrites DELETE 202
//! responses to 204 No Content.
//!
//! These tests exercise the rewrite layer hermetically against a stub
//! service (no docker stack); the live rmcp path is covered by
//! `crates/klams-service/tests/mcp_session_delete.rs`.

use axum::{
    body::{to_bytes, Body},
    http::{header, Method, Request, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use tower::ServiceExt;

/// Stub that answers every request the way rmcp's `handle_delete`
/// answers a successful session termination: 202 + empty body.
fn stub_router() -> Router {
    let svc = any(|| async {
        Response::builder()
            .status(StatusCode::ACCEPTED)
            .header(header::CONTENT_LENGTH, "0")
            .body(Body::empty())
            .unwrap()
    });
    Router::new()
        .route("/", svc)
        .layer(axum::middleware::from_fn(klams_mcp::delete_status_compat))
}

async fn send(method: Method) -> Response {
    stub_router()
        .oneshot(
            Request::builder()
                .method(method)
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn delete_202_is_rewritten_to_204_with_empty_body() {
    let resp = send(Method::DELETE).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    // 204 must not carry a body. (No Content-Length assertion here:
    // axum's Route re-adds `content-length: 0` for exact-size bodies
    // after router-level middleware; hyper strips it from 204s at the
    // wire, and the python-sdk only checks the status code.)
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert!(body.is_empty());
}

#[tokio::test]
async fn non_delete_202_passes_through_unchanged() {
    let resp = send(Method::POST).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn delete_with_non_202_status_passes_through() {
    // e.g. rmcp's 400 "Session ID is required" must stay a 400.
    let svc = any(|| async {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::from("Bad Request: Session ID is required"))
            .unwrap()
    });
    let router = Router::new()
        .route("/", svc)
        .layer(axum::middleware::from_fn(klams_mcp::delete_status_compat));
    let resp = router
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(resp.into_body(), 1024).await.unwrap();
    assert_eq!(&body[..], b"Bad Request: Session ID is required");
}
