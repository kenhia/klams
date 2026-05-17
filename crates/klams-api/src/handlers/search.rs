//! HTTP handler for `POST /memory/search`.
//!
//! Unified search across facts, events, and knowledge. For each
//! requested type (all three when `types` is omitted) the handler
//! issues the appropriate `Store` query in parallel, normalizes
//! per-type scores into `[0,1]`, and interleaves the results by
//! per-type rank up to `top_k`.
//!
//! Degraded mode: if the knowledge backend (Qdrant or the embedder)
//! returns an error we omit knowledge hits, set `degraded: true`,
//! log a warning, and still return 200 with the fact/event results.

use crate::router::ApiState;
use crate::ApiError;
use axum::{extract::State, Json};
use klams_core::metrics as m;
use klams_store::{Store, TextHit};
use klams_types::{KnowledgeItem, SearchHit, SearchRequest, SearchResults, SearchType};
use std::sync::Arc;
use tracing::warn;

const PREVIEW_MAX: usize = 200;

pub async fn search<S: Store>(
    State(state): State<ApiState<S>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResults>, ApiError> {
    let _guard = m::LatencyGuard::new(m::SEARCH_LATENCY);
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

    let store = Arc::clone(&state.store);
    let q_text = query.clone();
    let text_fut = async move {
        if want_facts || want_events {
            store.search_text(&q_text, top_k).await
        } else {
            Ok((Vec::<TextHit>::new(), Vec::<TextHit>::new()))
        }
    };

    let store2 = Arc::clone(&state.store);
    let q_vec = query.clone();
    let knowledge_fut = async move {
        if want_knowledge {
            let vec = store2.embed_query(&q_vec).await?;
            store2.search_knowledge(vec, top_k).await
        } else {
            Ok(Vec::new())
        }
    };

    let (text_res, knowledge_res) = tokio::join!(text_fut, knowledge_fut);

    let mut degraded = false;
    let (facts_hits, events_hits) = match text_res {
        Ok((f, e)) => (f, e),
        Err(err) => {
            warn!(error = %err, "search_text failed; continuing without text hits");
            degraded = true;
            (Vec::new(), Vec::new())
        }
    };
    let knowledge_hits: Vec<(KnowledgeItem, f32)> = if want_knowledge {
        match knowledge_res {
            Ok(v) => v,
            Err(err) => {
                warn!(error = %err, "knowledge search failed; degrading");
                degraded = true;
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    let facts_ranked = if want_facts {
        text_hits_to_search_hits(facts_hits, SearchType::Fact)
    } else {
        Vec::new()
    };
    let events_ranked = if want_events {
        text_hits_to_search_hits(events_hits, SearchType::Event)
    } else {
        Vec::new()
    };
    let knowledge_ranked = knowledge_hits_to_search_hits(knowledge_hits);

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

fn text_hits_to_search_hits(hits: Vec<TextHit>, kind: SearchType) -> Vec<SearchHit> {
    let max = hits.iter().map(|h| h.score).fold(0.0_f32, f32::max);
    hits.into_iter()
        .map(|h| {
            let norm = if max > 0.0 { h.score / max } else { 0.0 };
            let preview = preview_from_payload(&h.payload);
            SearchHit {
                kind,
                id: h.id,
                score: norm,
                preview,
                payload: h.payload,
            }
        })
        .collect()
}

fn knowledge_hits_to_search_hits(hits: Vec<(KnowledgeItem, f32)>) -> Vec<SearchHit> {
    hits.into_iter()
        .map(|(item, score)| {
            let preview = item.text.chars().take(PREVIEW_MAX).collect::<String>();
            SearchHit {
                kind: SearchType::Knowledge,
                id: item.id,
                score: score.clamp(0.0, 1.0),
                preview,
                payload: serde_json::to_value(&item).unwrap_or(serde_json::Value::Null),
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
