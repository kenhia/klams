//! HTTP handler for `POST /memory/context`.
//!
//! Sprint 005 (Phase 4) — returns a deduped, budget-respecting
//! `ContextBundle` of facts + knowledge + recent events for a query.
//!
//! Stub: returns `501 Not Implemented` until T020 lands the real
//! handler.

use crate::router::ApiState;
use crate::ApiError;
use axum::extract::State;
use klams_store::Store;

pub async fn context<S: Store>(State(_state): State<ApiState<S>>) -> Result<(), ApiError> {
    Err(ApiError::NotImplemented {
        what: "POST /memory/context is not implemented yet (sprint 005)".into(),
    })
}
