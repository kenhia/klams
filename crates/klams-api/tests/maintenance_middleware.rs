//! Sprint 006 T035/T036 (US3) — maintenance middleware integration tests.
//!
//! While `MaintenanceState::active()` is true, non-critical writes
//! return `503 + Retry-After`; reads and `CriticalWrite`-marked
//! routes pass through.

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use axum::{Extension, Router};
use klams_api::middleware::maintenance::{maintenance_check, CriticalWrite};
use klams_types::{MaintenanceState, RunningSnapshot};
use tower::ServiceExt;
use ulid::Ulid;

async fn ok_handler() -> &'static str {
    "ok"
}

fn build_app(state: MaintenanceState) -> Router {
    Router::new()
        .route("/memory/facts", post(ok_handler).get(ok_handler))
        .route("/memory/search", post(ok_handler))
        .route("/memory/context", post(ok_handler))
        .route("/memory/events", post(ok_handler))
        .route(
            "/memory/dissents/{id}/promote",
            post(ok_handler).route_layer(Extension(CriticalWrite)),
        )
        .route(
            "/memory/dissents/{id}/discard",
            post(ok_handler).route_layer(Extension(CriticalWrite)),
        )
        .layer(from_fn_with_state(state, maintenance_check))
}

fn mark_active(state: &MaintenanceState) {
    state.mark_active(RunningSnapshot {
        run_id: Ulid::new(),
        started_at: chrono::Utc::now(),
        expected_end_at: None,
    });
}

async fn status_of(app: &Router, method: Method, path: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let code = resp.status();
    let body = to_bytes(resp.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    (code, body)
}

#[tokio::test]
async fn inactive_state_passes_all_requests() {
    let state = MaintenanceState::new();
    let app = build_app(state);

    for (m, p) in [
        (Method::GET, "/memory/facts"),
        (Method::POST, "/memory/facts"),
        (Method::POST, "/memory/search"),
        (Method::POST, "/memory/context"),
        (Method::POST, "/memory/events"),
    ] {
        let (code, _) = status_of(&app, m.clone(), p).await;
        assert_eq!(code, StatusCode::OK, "{m} {p} should pass when inactive");
    }
}

#[tokio::test]
async fn active_state_short_circuits_writes_with_503_envelope() {
    let state = MaintenanceState::new();
    mark_active(&state);
    let app = build_app(state);

    let (code, body) = status_of(&app, Method::POST, "/memory/facts").await;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);

    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["error"], "maintenance_window_active");
    assert!(v["retry_after_seconds"].is_number());
    assert!(v["retry_after_seconds"].as_u64().unwrap() >= 30);
}

#[tokio::test]
async fn active_state_lets_reads_pass() {
    let state = MaintenanceState::new();
    mark_active(&state);
    let app = build_app(state);
    let (code, _) = status_of(&app, Method::GET, "/memory/facts").await;
    assert_eq!(code, StatusCode::OK);
}

#[tokio::test]
async fn active_state_lets_search_and_context_pass() {
    // /memory/search and /memory/context are POST but qualify as
    // reads (FR-007); per research.md R-005 they're tagged as
    // CriticalWrite=false but the middleware whitelists them by path.
    let state = MaintenanceState::new();
    mark_active(&state);
    let app = build_app(state);

    let (s, _) = status_of(&app, Method::POST, "/memory/search").await;
    assert_eq!(s, StatusCode::OK, "search must pass during maintenance");
    let (c, _) = status_of(&app, Method::POST, "/memory/context").await;
    assert_eq!(c, StatusCode::OK, "context must pass during maintenance");
}

#[tokio::test]
async fn active_state_lets_critical_write_pass() {
    let state = MaintenanceState::new();
    mark_active(&state);
    let app = build_app(state);

    let (code, _) = status_of(&app, Method::POST, "/memory/dissents/abc/promote").await;
    assert_eq!(
        code,
        StatusCode::OK,
        "CriticalWrite-path route must pass during maintenance"
    );
    let (code, _) = status_of(&app, Method::POST, "/memory/dissents/abc/discard").await;
    assert_eq!(code, StatusCode::OK, "discard must also pass");
}

#[tokio::test]
async fn retry_after_header_present_on_503() {
    let state = MaintenanceState::new();
    mark_active(&state);
    let app = build_app(state);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/memory/facts")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let hdr = resp
        .headers()
        .get("retry-after")
        .expect("retry-after header")
        .to_str()
        .unwrap()
        .to_string();
    let n: u64 = hdr.parse().expect("integer seconds");
    assert!(n >= 30, "retry-after floor is 30s, got {n}");
}
