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

    let facts_ranked = ranked_to_hits(
        facts_rows,
        SearchType::Fact,
        /*from_payload_text=*/ false,
    );
    let events_ranked = ranked_to_hits(events_rows, SearchType::Event, false);
    let knowledge_ranked = ranked_to_hits(vector_rows, SearchType::Knowledge, true);

    let merged = interleave(
        &[facts_ranked, events_ranked, knowledge_ranked],
        top_k as usize,
    );

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

fn ranked_to_hits(
    rows: Vec<RankedRow>,
    kind: SearchType,
    text_from_payload: bool,
) -> Vec<SearchHit> {
    rows.into_iter()
        .map(|r| {
            let preview = if text_from_payload {
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
        })
        .collect()
}

fn preview_from_payload(payload: &serde_json::Value) -> String {
    let s = payload.to_string();
    s.chars().take(PREVIEW_MAX).collect()
}

/// Round-robin merge: pull one hit from each non-empty list in order
/// until `cap` items are gathered. Within each list, original rank
/// order (highest score first) is preserved.
fn interleave(buckets: &[Vec<SearchHit>], cap: usize) -> Vec<SearchHit> {
    let mut idx = vec![0usize; buckets.len()];
    let mut out: Vec<SearchHit> = Vec::with_capacity(cap);
    while out.len() < cap {
        let mut pushed = false;
        for (b, bucket) in buckets.iter().enumerate() {
            if idx[b] < bucket.len() {
                out.push(bucket[idx[b]].clone());
                idx[b] += 1;
                pushed = true;
                if out.len() >= cap {
                    break;
                }
            }
        }
        if !pushed {
            break;
        }
    }
    out
}
