//! HTTP handler for `POST /memory/context`.
//!
//! Sprint 005 (Phase 4) — T020. Deserializes a `ContextRequest`,
//! wraps the configured `Store` in a `StoreHybridAdapter`, runs the
//! `ContextBuilder`, and returns a `ContextBundle`. Per-section
//! degradation is reported in `sections[*].status`; only when
//! *every* source is unavailable do we map to a 503 (FR-011).
//!
//! Filter-key validation: serde's `deny_unknown_fields` is enforced
//! at the `RetrievalFilters` boundary (see `klams_types::context`),
//! so unknown keys surface as 400 with the offending key named by
//! the serde error message.

use crate::router::ApiState;
use crate::ApiError;
use axum::{extract::State, Json};
use klams_core::context::BuildError;
use klams_core::hybrid::StoreHybridAdapter;
use klams_core::metrics as m;
use klams_store::Store;
use klams_types::{ContextBundle, ContextRequest};
use std::sync::Arc;
use tracing::warn;

const ALL_SOURCES_UNAVAILABLE_RETRY_AFTER: u32 = 5;

pub async fn context<S: Store>(
    State(state): State<ApiState<S>>,
    Json(req): Json<ContextRequest>,
) -> Result<Json<ContextBundle>, ApiError> {
    let _guard = m::LatencyGuard::retrieval("context", "rest");

    let query = req.query.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::Validation {
            field: "query".into(),
            message: "query_required".into(),
        });
    }

    let hybrid = StoreHybridAdapter::new(Arc::clone(&state.store));
    let normalized = ContextRequest {
        query,
        token_budget: req.token_budget,
        filters: req.filters,
    };

    let bundle = match state.context_builder.build(&hybrid, &normalized).await {
        Ok(b) => b,
        Err(BuildError::AllSourcesUnavailable) => {
            warn!("all retrieval sources unavailable; returning 503");
            return Err(ApiError::AllSourcesUnavailable {
                retry_after: ALL_SOURCES_UNAVAILABLE_RETRY_AFTER,
            });
        }
    };

    m::incr_context_section_items("facts", bundle.facts.len() as u64);
    m::incr_context_section_items("knowledge", bundle.knowledge.len() as u64);
    m::incr_context_section_items("events", bundle.events.len() as u64);

    Ok(Json(bundle))
}
