//! `memory_search` MCP tool (sprint 007 T037, US2).
//!
//! Since sprint 036 (#730) this is a thin shell: argument parsing +
//! the MCP error envelope around the shared retrieval pipeline in
//! [`klams_core::retrieval`]. The pipeline itself — curated stratum,
//! provenance weights, boost gate, duplicate collapse, second-stage
//! rerank, weighted RRF, miss + sample logs — lives in the core so
//! REST `/memory/search` runs the identical code. This tool keeps the
//! MCP contract: hard-fail on any source error, with the 027
//! transient/permanent taxonomy on the wire.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    metrics as mcp_metrics,
    tools::McpState,
};
use klams_core::retrieval::{self, RetrievalConfig, RetrievalError, SourceTolerance};
use klams_store::Store;
use klams_types::{MemoryKind, RetrievalFilters, ScoredMemory};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// Shared with `event_search` (033, #692) — one label helper so the
// miss log, sample log, and metric counters can never disagree.
pub(crate) use klams_core::retrieval::caller_label;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKindFilter {
    Fact,
    Knowledge,
    Event,
}

impl From<MemoryKindFilter> for MemoryKind {
    fn from(v: MemoryKindFilter) -> Self {
        match v {
            MemoryKindFilter::Fact => MemoryKind::Fact,
            MemoryKindFilter::Knowledge => MemoryKind::Knowledge,
            MemoryKindFilter::Event => MemoryKind::Event,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorySearchArgs {
    pub query: String,
    #[serde(default)]
    pub kinds: Option<Vec<MemoryKindFilter>>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub top_k: Option<u32>,
    /// Return whole memory texts inline instead of compact snippets.
    ///
    /// Sprint 046 (WI #1178): compact is the default. Full text cost
    /// 4,491 tokens per answered query on khound's fair suite; the
    /// compact contract cost 1,024-1,476 on the same suite with answers
    /// preserved, and that metric already charges the follow-up read
    /// when a snippet fell short. Default-on is the deliberate call —
    /// a flag nobody passes is a default that never changes anything —
    /// so this exists for the callers that genuinely want bodies in
    /// bulk (eval harnesses, exports), not as the ordinary path.
    #[serde(default)]
    pub full: Option<bool>,
}

/// What `memory_search` returns: compact hits by default, the full
/// ranked list when the caller asked for it.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MemorySearchOutput {
    Compact(crate::contract::CompactSearchResponse),
    Full(Vec<ScoredMemory>),
}

impl MemorySearchOutput {
    /// The full ranked list, when the caller asked for `full: true`.
    ///
    /// `None` for a compact response — deliberately not a silent
    /// projection back to full records, because there are none to
    /// project: the bodies were never fetched into this shape.
    #[must_use]
    pub fn into_full(self) -> Option<Vec<ScoredMemory>> {
        match self {
            Self::Full(hits) => Some(hits),
            Self::Compact(_) => None,
        }
    }

    /// The compact response, when the caller took the default.
    #[must_use]
    pub fn into_compact(self) -> Option<crate::contract::CompactSearchResponse> {
        match self {
            Self::Compact(resp) => Some(resp),
            Self::Full(_) => None,
        }
    }
}

/// Execute `memory_search`. Returns the merged ranked hits on success
/// or an MCP error envelope otherwise.
///
/// Sprint 046 (WI #1178): the default shape is now COMPACT — a
/// match-window snippet plus a locator per hit, and a `more.fetch`
/// pointer at `memory_get`. Pass `full: true` for the previous shape:
/// [`ScoredMemory`] values carrying the whole text, the fused `score`
/// (RRF, #328), the per-source `source_rank` and the pre-fusion
/// `raw_score` (#332).
///
/// The compact hits carry `score`, `raw_score` and `source_rank` too,
/// so a retrieval eval can still see how a hit ranked and how well it
/// matched without asking for bodies.
///
/// **Scope of the change:** this is the MCP tool only. REST
/// `/v1/search` is untouched and still returns full records — it has
/// non-agent consumers whose contract nothing here justifies breaking,
/// and the token argument that motivates compactness is an agent-context
/// argument.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `EMPTY_QUERY`, `INVALID_TOP_K`,
/// `EMBEDDING_UNAVAILABLE`, or `INTERNAL_ERROR`.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: MemorySearchArgs,
    caller: Option<&str>,
) -> Result<MemorySearchOutput, ErrorEnvelope> {
    let full = args.full.unwrap_or(false);
    // Kept for the snippet window: the compact projection needs the
    // query to know which part of a long memory to cut.
    let query_for_snippets = args.query.clone();
    let params = retrieval::SearchParams {
        query: args.query,
        kinds: args
            .kinds
            .map(|v| v.into_iter().map(MemoryKind::from).collect()),
        tags: args.tags,
        filters: RetrievalFilters::default(),
        top_k: args.top_k,
    };
    let config = RetrievalConfig {
        fusion: state.fusion,
        reranker: state.reranker.clone(),
        rerank_window: state.rerank_window,
    };
    let outcome = retrieval::search(
        &state.store,
        params,
        &config,
        SourceTolerance::HardFail,
        caller,
        "mcp",
    )
    .await
    .map_err(map_error)?;

    // Sprint 026 (#643): attribute the caller. This was hardcoded
    // `"anonymous"` since sprint 020, so the per-agent search counter had
    // exactly one label value and answered no question it was added to
    // answer — while the caller's agent name was sitting right there in
    // the argument list.
    mcp_metrics::record_search(caller_label(caller), None);
    if full {
        return Ok(MemorySearchOutput::Full(outcome.hits));
    }
    Ok(MemorySearchOutput::Compact(
        crate::contract::CompactSearchResponse::build(&outcome.hits, &query_for_snippets),
    ))
}

/// Map a neutral [`RetrievalError`] onto the MCP error envelope,
/// preserving the sprint-027 transient/permanent embedding taxonomy
/// (`from_store_error` decides `retry_after_seconds`).
pub(crate) fn map_error(e: RetrievalError) -> ErrorEnvelope {
    match e {
        RetrievalError::EmptyQuery => envelope(errors::EMPTY_QUERY, "query must be non-empty"),
        RetrievalError::QueryTooLong { max } => envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("query exceeds {max} characters"),
        ),
        RetrievalError::InvalidTopK { max } => {
            envelope(errors::INVALID_TOP_K, format!("top_k must be 1..={max}"))
        }
        RetrievalError::Embed(store_err) => errors::from_store_error("embed_query", &store_err),
        RetrievalError::Backend { context, source } => {
            envelope(errors::INTERNAL_ERROR, format!("{context}: {source}"))
        }
        RetrievalError::NotFound(msg) => envelope(errors::NOT_FOUND, msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The pipeline's behavioral tests (fusion, collapse, rerank order,
    // miss classification, filters) moved to `klams_core::retrieval`
    // with the code in sprint 036 (#730). What belongs to THIS shell is
    // the error mapping: the envelope codes are the MCP wire contract.

    #[test]
    fn validation_errors_map_to_their_canonical_codes() {
        let v = serde_json::to_value(map_error(RetrievalError::EmptyQuery)).unwrap();
        assert_eq!(v["_meta"]["error_code"], "EMPTY_QUERY");
        let v =
            serde_json::to_value(map_error(RetrievalError::QueryTooLong { max: 1024 })).unwrap();
        assert_eq!(v["_meta"]["error_code"], "SCHEMA_VALIDATION_FAILED");
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("1024 characters"));
        let v = serde_json::to_value(map_error(RetrievalError::InvalidTopK { max: 50 })).unwrap();
        assert_eq!(v["_meta"]["error_code"], "INVALID_TOP_K");
        assert!(v["content"][0]["text"].as_str().unwrap().contains("1..=50"));
    }

    #[test]
    fn a_transient_embed_error_keeps_its_retry_hint() {
        // The 027 contract: a down embedder is retryable and says so.
        let e = RetrievalError::Embed(klams_store::StoreError::Embedding("down".into()));
        let v = serde_json::to_value(map_error(e)).unwrap();
        assert_eq!(v["_meta"]["error_code"], "EMBEDDING_UNAVAILABLE");
        assert!(v["_meta"]["retry_after_seconds"].is_number());
    }

    #[test]
    fn an_oversized_query_embed_error_is_permanent() {
        // PAYLOAD_TOO_LARGE must never carry a retry hint — retrying
        // the same oversized text can never succeed (the 027 bug).
        let oversize = klams_types::EmbedLimit::default()
            .check(&"a".repeat(9000))
            .expect_err("9000 chars must exceed the default ceiling");
        let e = RetrievalError::Embed(klams_store::StoreError::PayloadTooLarge {
            oversize,
            detail: "test".into(),
        });
        let v = serde_json::to_value(map_error(e)).unwrap();
        assert_eq!(v["_meta"]["error_code"], "PAYLOAD_TOO_LARGE");
        assert!(v["_meta"].get("retry_after_seconds").is_none());
    }

    #[test]
    fn backend_errors_name_their_context() {
        let e = RetrievalError::Backend {
            context: "search_knowledge",
            source: klams_store::StoreError::Other("boom".into()),
        };
        let v = serde_json::to_value(map_error(e)).unwrap();
        assert_eq!(v["_meta"]["error_code"], "INTERNAL_ERROR");
        assert!(v["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("search_knowledge:"));
    }
}
