//! HTTP handler for `POST /memory/search`.
//!
//! Sprint 036 (#730): runs the SAME pipeline as MCP `memory_search` —
//! [`klams_core::retrieval::search`] — instead of the old
//! `StoreHybridAdapter` path. That ends five years of quiet divergence
//! in one move: REST results now get the curated stratum, the
//! three-tier (author-aware) provenance weights, the second-stage
//! rerank, the configured `[retrieval] fusion` (previously hardcoded
//! RRF), separate fact/event budgets (previously one shared budget
//! truncated before the split), the 1024-char query cap, and the
//! miss/sample logs with caller attribution.
//!
//! The REST contract this shell preserves: `top_k` is clamped rather
//! than rejected, and source failures degrade instead of erroring —
//! [`SourceTolerance::Degrade`] omits the sick source, sets
//! `degraded: true`, and still returns 200 with what the healthy
//! sources produced.
//!
//! Wire compatibility: `SearchHit.payload` keeps the adapter-era keys
//! (`section`, and for knowledge `kind`/`text`/`file`/`tags`/`repo`/
//! `host`/`content_hash`/`heading_path`/`language`/`chunk_index`, plus
//! `copies` when duplicates collapsed). Additively new: `author` and
//! `created_at` in payloads, and `raw_score`/`source_rank` on the hit.
//! `score` is now the fused RRF value both surfaces report.

use crate::router::ApiState;
use crate::ApiError;
use axum::{extract::State, Extension, Json};
use klams_core::retrieval::{self, RetrievalConfig, RetrievalError, SourceTolerance};
use klams_store::Store;
use klams_types::{
    AuthenticatedAuthor, ItemKind, MemoryKind, PublicMemoryContent, RetrievalFilters, ScoredMemory,
    SearchHit, SearchRequest, SearchResults, SearchType,
};

const PREVIEW_MAX: usize = 200;

pub async fn search<S: Store>(
    State(state): State<ApiState<S>>,
    author: Option<Extension<AuthenticatedAuthor>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResults>, ApiError> {
    // Sprint 033 (#692): `filters` was accepted and silently discarded
    // since sprint 005 — every search ran with the defaults. Parse it
    // into the same `RetrievalFilters` the `/memory/context` handler
    // uses, so both surfaces share one filter contract.
    let filters = match req.filters.clone() {
        Some(v) => {
            serde_json::from_value::<RetrievalFilters>(v).map_err(|e| ApiError::Validation {
                field: "filters".into(),
                message: format!("invalid filters: {e}"),
            })?
        }
        None => RetrievalFilters::default(),
    };
    let params = retrieval::SearchParams {
        query: req.query.clone(),
        kinds: req
            .types
            .as_ref()
            .map(|v| v.iter().map(|t| kind_of(*t)).collect()),
        tags: None,
        filters,
        // REST clamps rather than rejects (the sprint-005 contract);
        // the ceiling is the pipeline's MAX_TOP_K (50 — down from the
        // old handler's 100, which the divergent path never actually
        // honored past its own over-fetch anyway).
        top_k: Some(req.top_k.clamp(1, retrieval::MAX_TOP_K)),
    };
    let config = RetrievalConfig {
        fusion: state.fusion,
        reranker: state.reranker.clone(),
        rerank_window: state.rerank_window,
    };
    // Sprint 036: attribute the caller (the miss/sample logs and the
    // per-agent counters were MCP-only before). The bearer layer stamps
    // the extension for author-bound grants.
    let caller = author.as_ref().map(|Extension(a)| a.agent_name.as_str());
    let outcome = retrieval::search(
        &state.store,
        params,
        &config,
        SourceTolerance::Degrade,
        caller,
        "rest",
    )
    .await
    .map_err(map_error)?;

    let results: Vec<SearchHit> = outcome.hits.into_iter().map(scored_to_hit).collect();
    Ok(Json(SearchResults {
        total: results.len(),
        results,
        query: req.query,
        degraded: outcome.degraded,
    }))
}

fn kind_of(t: SearchType) -> MemoryKind {
    match t {
        SearchType::Fact => MemoryKind::Fact,
        SearchType::Event => MemoryKind::Event,
        SearchType::Knowledge => MemoryKind::Knowledge,
    }
}

/// Map a neutral [`RetrievalError`] onto the REST wire. Under
/// [`SourceTolerance::Degrade`] only the validation variants can
/// actually surface; the backend arms are kept for completeness.
fn map_error(e: RetrievalError) -> ApiError {
    match e {
        RetrievalError::EmptyQuery => ApiError::Validation {
            field: "query".into(),
            message: "query must be non-empty".into(),
        },
        RetrievalError::QueryTooLong { max } => ApiError::Validation {
            field: "query".into(),
            message: format!("query exceeds {max} characters"),
        },
        RetrievalError::InvalidTopK { max } => ApiError::Validation {
            field: "top_k".into(),
            message: format!("top_k must be 1..={max}"),
        },
        RetrievalError::NotFound(resource) => ApiError::NotFound { resource },
        other => ApiError::Internal {
            request_id: format!("retrieval-error: {other}"),
        },
    }
}

/// Project a pipeline [`ScoredMemory`] to the REST [`SearchHit`],
/// rebuilding the adapter-era payload keys so existing consumers keep
/// parsing what they always parsed.
fn scored_to_hit(sm: ScoredMemory) -> SearchHit {
    let mem = sm.memory;
    let author = serde_json::json!(mem.author.agent_name);
    let created_at = serde_json::json!(mem.created_at);
    let (kind, preview, payload) = match mem.content {
        PublicMemoryContent::Knowledge {
            text,
            source_path,
            repo,
            host,
            content_hash,
            heading_path,
            language,
            chunk_index,
            copies,
            volatility,
            ..
        } => {
            let preview = text.chars().take(PREVIEW_MAX).collect::<String>();
            let mut payload = serde_json::json!({
                "kind": ItemKind::Raw,
                "section": "knowledge",
                "text": text,
                "file": source_path,
                "tags": mem.tags,
                "repo": repo,
                "host": host,
                "content_hash": content_hash,
                "heading_path": heading_path,
                "language": language,
                "chunk_index": chunk_index,
                "author": author,
                "created_at": created_at,
                "volatility": volatility,
            });
            // Same rule as the adapter had: no empty `copies` array on
            // the wire for a result that absorbed nothing.
            if !copies.is_empty() {
                if let Some(obj) = payload.as_object_mut() {
                    obj.insert(
                        "copies".into(),
                        serde_json::to_value(&copies).unwrap_or(serde_json::Value::Null),
                    );
                }
            }
            (SearchType::Knowledge, preview, payload)
        }
        PublicMemoryContent::Fact { fact_type, payload } => {
            let mut out = payload;
            if let Some(obj) = out.as_object_mut() {
                obj.insert("section".into(), serde_json::json!("facts"));
                obj.insert("type".into(), serde_json::json!(fact_type));
                obj.insert("author".into(), author);
                obj.insert("created_at".into(), created_at);
            }
            (SearchType::Fact, preview_of(&out), out)
        }
        PublicMemoryContent::Event {
            category,
            payload,
            task_id,
        } => {
            let mut out = payload;
            if let Some(obj) = out.as_object_mut() {
                obj.insert("section".into(), serde_json::json!("events"));
                obj.insert("category".into(), serde_json::json!(category));
                obj.insert("task_id".into(), serde_json::json!(task_id));
                obj.insert("author".into(), author);
                obj.insert("created_at".into(), created_at);
            }
            (SearchType::Event, preview_of(&out), out)
        }
    };
    SearchHit {
        kind,
        id: mem.id,
        score: sm.score,
        preview,
        payload,
        raw_score: sm.raw_score,
        source_rank: Some(sm.source_rank),
    }
}

fn preview_of(payload: &serde_json::Value) -> String {
    payload.to_string().chars().take(PREVIEW_MAX).collect()
}
