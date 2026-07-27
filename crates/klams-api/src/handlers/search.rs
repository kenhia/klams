//! HTTP handler for `POST /memory/search`.
//!
//! Sprint 005 (T029): the per-type fan-out now runs through the
//! `HybridStore` adapter (`StoreHybridAdapter`) so search shares the
//! same retrieval plumbing as `/memory/context`. The response shape
//! (`SearchResults` + `SearchHit`) is preserved; knowledge hits now
//! carry the slimmer hybrid payload (`{kind, section, text, file,
//! tags, repo}`) instead of the full `KnowledgeItem` row.
//!
//! Degraded mode: if the vector retrieve fails (Qdrant or the
//! embedder) we omit knowledge hits, set `degraded: true`, log a
//! warning, and still return 200 with the fact/event results.

use crate::router::ApiState;
use crate::ApiError;
use axum::{extract::State, Json};
use klams_core::hybrid::StoreHybridAdapter;
use klams_core::metrics as m;
use klams_store::{HybridStore, RankedRow, Store};
use klams_types::{
    RetrievalFilters, RetrievalSource, SearchHit, SearchRequest, SearchResults, SearchType,
};
use std::sync::Arc;
use tracing::warn;

const PREVIEW_MAX: usize = 200;

pub async fn search<S: Store>(
    State(state): State<ApiState<S>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResults>, ApiError> {
    let _guard = m::LatencyGuard::retrieval("search", "rest");
    let query = req.query.trim().to_string();
    if query.is_empty() {
        return Err(ApiError::Validation {
            field: "query".into(),
            message: "query must be non-empty".into(),
        });
    }
    let top_k = req.top_k.clamp(1, 100);
    let want_facts = wants(req.types.as_ref(), SearchType::Fact);
    let want_events = wants(req.types.as_ref(), SearchType::Event);
    let want_knowledge = wants(req.types.as_ref(), SearchType::Knowledge);

    let adapter = StoreHybridAdapter::new(Arc::clone(&state.store));
    let filters = RetrievalFilters::default();

    let vector_fut = async {
        if want_knowledge {
            adapter
                .retrieve(RetrievalSource::Vector, &query, &filters, top_k)
                .await
        } else {
            Ok(Vec::<RankedRow>::new())
        }
    };
    let fts_fut = async {
        if want_facts || want_events {
            adapter
                .retrieve(RetrievalSource::Fts, &query, &filters, top_k)
                .await
        } else {
            Ok(Vec::<RankedRow>::new())
        }
    };
    let (vector_res, fts_res) = tokio::join!(vector_fut, fts_fut);

    let mut degraded = false;
    let vector_rows = match vector_res {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "knowledge retrieve failed; degrading");
            degraded = true;
            Vec::new()
        }
    };
    let fts_rows = match fts_res {
        Ok(v) => v,
        Err(err) => {
            warn!(error = %err, "fts retrieve failed; continuing without text hits");
            degraded = true;
            Vec::new()
        }
    };

    let mut facts_rows: Vec<RankedRow> = Vec::new();
    let mut events_rows: Vec<RankedRow> = Vec::new();
    for r in fts_rows {
        match r.payload.get("section").and_then(|v| v.as_str()) {
            Some("facts") if want_facts => facts_rows.push(r),
            Some("events") if want_events => events_rows.push(r),
            _ => {}
        }
    }

    // Sprint 024 (#328): converge on the same RRF rank fusion the MCP
    // `memory_search` and `/memory/context` paths use, instead of the
    // old per-source round-robin. Each list is already ranked best-first;
    // `hybrid::fuse` keys on id + within-source rank, so no source
    // structurally outranks another. Kind is recovered from the payload
    // `section` when projecting each fused row to a `SearchHit`.
    let fused = klams_core::hybrid::fuse(
        vec![vector_rows, facts_rows, events_rows],
        klams_types::FusionStrategy::default_rrf(),
    );
    let merged: Vec<SearchHit> = fused
        .into_iter()
        .take(top_k as usize)
        .map(ranked_row_to_hit)
        .collect();

    Ok(Json(SearchResults {
        total: merged.len(),
        results: merged,
        query: req.query,
        degraded,
    }))
}

fn wants(types: Option<&Vec<SearchType>>, t: SearchType) -> bool {
    match types {
        None => true,
        Some(v) => v.contains(&t),
    }
}

/// Project a fused [`RankedRow`] to a [`SearchHit`], recovering the
/// result kind from the payload `section` the hybrid adapter stamped
/// (`knowledge` / `facts` / `events`). Knowledge previews from the
/// payload `text`; facts/events preview the serialized payload.
fn ranked_row_to_hit(r: RankedRow) -> SearchHit {
    let section = r
        .payload
        .get("section")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let (kind, from_text) = match section {
        "knowledge" => (SearchType::Knowledge, true),
        "events" => (SearchType::Event, false),
        _ => (SearchType::Fact, false),
    };
    let preview = if from_text {
        r.payload
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.chars().take(PREVIEW_MAX).collect::<String>())
            .unwrap_or_default()
    } else {
        preview_from_payload(&r.payload)
    };
    SearchHit {
        kind,
        id: r.id,
        score: r.score.clamp(0.0, 1.0),
        preview,
        payload: r.payload,
    }
}

fn preview_from_payload(payload: &serde_json::Value) -> String {
    let s = payload.to_string();
    s.chars().take(PREVIEW_MAX).collect()
}
