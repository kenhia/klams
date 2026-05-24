//! Maintenance-window middleware: 503 + Retry-After on non-critical
//! writes while `MaintenanceState::active()` is true (sprint 006
//! T037/T038).
//!
//! Per FR-007 the following pass through even when active:
//!   * any `GET` request (reads),
//!   * `POST /memory/search` and `POST /memory/context` (POSTs purely
//!     because their bodies are too large for a URL),
//!   * any handler tagged with the [`CriticalWrite`] route extension
//!     (currently User-source dissent transitions per FR-008).
//!
//! `retry_after_seconds` is `max(30, expected_end_at - now)` per
//! research.md R-005.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use klams_types::MaintenanceState;
use serde_json::json;

/// Route-extension marker. Attach via
/// `.route_layer(Extension(CriticalWrite))` on endpoints that must
/// keep accepting writes during the backup window.
#[derive(Debug, Clone, Copy)]
pub struct CriticalWrite;

/// Lowest acceptable `Retry-After` value (seconds). Protects clients
/// from hot-looping when no duration projection is available.
const RETRY_AFTER_FLOOR_SECONDS: u64 = 30;

/// Re-exported for `router.rs` to call
/// `axum::middleware::from_fn_with_state(state, maintenance_check)`.
pub async fn maintenance_check(
    State(state): State<MaintenanceState>,
    req: Request,
    next: Next,
) -> Response {
    if !state.active() || is_pass_through(&req) {
        return next.run(req).await;
    }
    rejection_response(&state)
}

fn is_pass_through(req: &Request) -> bool {
    if req.method() == Method::GET {
        return true;
    }
    if req.extensions().get::<CriticalWrite>().is_some() {
        return true;
    }
    // POSTed reads (search + context).
    let path = req.uri().path();
    if matches!(path, "/memory/search" | "/memory/context") {
        return true;
    }
    // Critical writes by path (FR-008): User-source dissent
    // transitions must keep flowing through the backup window.
    // Path-based match is necessary because axum applies global
    // `.layer(..)` before routing, so route-level extensions are not
    // yet visible. The path set is small and stable.
    is_critical_write_path(path)
}

fn is_critical_write_path(path: &str) -> bool {
    // `/memory/dissents/{id}/promote` and `.../discard`.
    let Some(rest) = path.strip_prefix("/memory/dissents/") else {
        return false;
    };
    let Some((_id, tail)) = rest.split_once('/') else {
        return false;
    };
    matches!(tail, "promote" | "discard")
}

fn rejection_response(state: &MaintenanceState) -> Response {
    let retry = retry_after_seconds(state);
    let body = Json(json!({
        "error": "maintenance_window_active",
        "retry_after_seconds": retry,
    }));
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, body).into_response();
    if let Ok(v) = HeaderValue::from_str(&retry.to_string()) {
        resp.headers_mut().insert("retry-after", v);
    }
    resp
}

fn retry_after_seconds(state: &MaintenanceState) -> u64 {
    let Some(snap) = state.inflight() else {
        return RETRY_AFTER_FLOOR_SECONDS;
    };
    let projected = snap
        .expected_end_at
        .map_or(0, |end| (end - Utc::now()).num_seconds());
    let projected = u64::try_from(projected.max(0)).unwrap_or(0);
    projected.max(RETRY_AFTER_FLOOR_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use klams_types::RunningSnapshot;
    use ulid::Ulid;

    #[test]
    fn floor_when_no_inflight() {
        let s = MaintenanceState::new();
        assert_eq!(retry_after_seconds(&s), RETRY_AFTER_FLOOR_SECONDS);
    }

    #[test]
    fn floor_when_expected_end_in_past() {
        let s = MaintenanceState::new();
        s.mark_active(RunningSnapshot {
            run_id: Ulid::new(),
            started_at: Utc::now() - chrono::Duration::seconds(120),
            expected_end_at: Some(Utc::now() - chrono::Duration::seconds(10)),
        });
        assert_eq!(retry_after_seconds(&s), RETRY_AFTER_FLOOR_SECONDS);
    }

    #[test]
    fn uses_projection_when_above_floor() {
        let s = MaintenanceState::new();
        let end = Utc::now() + chrono::Duration::seconds(120);
        s.mark_active(RunningSnapshot {
            run_id: Ulid::new(),
            started_at: Utc::now(),
            expected_end_at: Some(end),
        });
        let r = retry_after_seconds(&s);
        assert!((110..=130).contains(&r), "expected ~120, got {r}");
    }
}
