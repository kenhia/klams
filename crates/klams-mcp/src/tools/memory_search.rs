//! `memory_search` MCP tool (sprint 007 T037, US2).
//!
//! Federates Postgres FTS (facts + events) and Qdrant ANN (knowledge)
//! into a single ranked list of [`ScoredMemory`] envelopes (sprint 016
//! — each wraps a [`PublicMemory`] projection with its relevance
//! `score` and per-source rank). Soft-deleted rows / points are
//! excluded by the underlying store helpers (`fetch_facts_with_authors`
//! filters `deleted_at IS NULL`; `QdrantStore::search_knowledge`
//! filters `is_empty("deleted_at")`).
//!
//! Optional `kinds` filter narrows which backends are queried.
//! Optional `tags` filter is applied in-memory after projection so the
//! same filter works for facts (which carry no tags column) and
//! knowledge (which does). Result count is clamped to `top_k`.
//!
//! Sprint 016 caveat: `score` is raw and *not* normalized across kinds
//! (knowledge = Qdrant cosine, fact/event = Postgres `ts_rank`), so the
//! merged ordering is biased toward knowledge. Exposed, not corrected.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    metrics as mcp_metrics, projection,
    tools::McpState,
};
use klams_types::{MemoryKind, PublicAuthorRef, PublicMemory, ScoredMemory};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULT_TOP_K: u32 = 10;
const MAX_TOP_K: u32 = 50;
const MAX_QUERY_LEN: usize = 1024;

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
}

/// Execute `memory_search`. Returns the merged ranked hits on success
/// or an MCP error envelope otherwise. Sprint 016: each result is a
/// [`ScoredMemory`] carrying the relevance `score` and per-source
/// `source_rank` alongside the memory, so retrieval evals can see why
/// a hit ranked where it did. Scores are raw and not cross-kind
/// comparable — see [`ScoredMemory`]'s scale caveat.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `EMPTY_QUERY`, `INVALID_TOP_K`,
/// `EMBEDDING_UNAVAILABLE`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run(
    state: &McpState,
    args: MemorySearchArgs,
) -> Result<Vec<ScoredMemory>, ErrorEnvelope> {
    let query = args.query.trim().to_string();
    if query.is_empty() {
        return Err(envelope(errors::EMPTY_QUERY, "query must be non-empty"));
    }
    if query.len() > MAX_QUERY_LEN {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("query exceeds {MAX_QUERY_LEN} characters"),
        ));
    }
    let top_k = args.top_k.unwrap_or(DEFAULT_TOP_K);
    if top_k == 0 || top_k > MAX_TOP_K {
        return Err(envelope(
            errors::INVALID_TOP_K,
            format!("top_k must be 1..={MAX_TOP_K}"),
        ));
    }

    let kinds: Vec<MemoryKind> = args.kinds.map_or_else(
        || vec![MemoryKind::Fact, MemoryKind::Knowledge, MemoryKind::Event],
        |v| v.into_iter().map(MemoryKind::from).collect(),
    );
    let want_fact = kinds.contains(&MemoryKind::Fact);
    let want_knowledge = kinds.contains(&MemoryKind::Knowledge);
    let want_event = kinds.contains(&MemoryKind::Event);

    // Each entry: (score, source_rank, memory). `source_rank` is the
    // hit's 0-based position within its own source's result list,
    // captured before the cross-source merge below.
    let mut scored: Vec<(f32, u32, PublicMemory)> = Vec::new();

    // ---------- knowledge via Qdrant ANN ----------
    if want_knowledge {
        let embedding = state.store.embedder.embed(&query).await.map_err(|e| {
            crate::errors::envelope_with_retry(
                errors::EMBEDDING_UNAVAILABLE,
                format!("TEI embedding failed: {e}"),
                5,
            )
        })?;
        let hits = state
            .store
            .qdrant
            .search_knowledge(embedding, top_k)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("search_knowledge: {e}")))?;
        if !hits.is_empty() {
            let ids: Vec<uuid::Uuid> = hits.iter().map(|(it, _)| it.id).collect();
            let author_map = state
                .store
                .qdrant
                .knowledge_authors_by_ids(&ids)
                .await
                .unwrap_or_default();
            let mut wanted_authors: Vec<uuid::Uuid> = author_map.values().copied().collect();
            wanted_authors.sort();
            wanted_authors.dedup();
            let authors = fetch_authors(state, &wanted_authors).await;
            // Qdrant returns hits already ordered by descending
            // similarity, so enumerate() yields the source rank.
            for (rank, (item, score)) in hits.into_iter().enumerate() {
                let author_ref = author_map
                    .get(&item.id)
                    .and_then(|aid| authors.get(aid))
                    .cloned()
                    .unwrap_or_else(unknown_author_ref);
                scored.push((
                    score,
                    rank_u32(rank),
                    projection::project_knowledge(&item, author_ref),
                ));
            }
        }
    }

    // ---------- facts + events via Postgres FTS ----------
    if want_fact || want_event {
        let (fact_hits, event_hits) = state
            .store
            .postgres
            .search_text(&query, top_k)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("search_text: {e}")))?;
        if want_fact && !fact_hits.is_empty() {
            let ids: Vec<uuid::Uuid> = fact_hits.iter().map(|h| h.id).collect();
            let rows = state
                .store
                .postgres
                .fetch_facts_with_authors(&ids)
                .await
                .map_err(|e| {
                    envelope(
                        errors::INTERNAL_ERROR,
                        format!("fetch_facts_with_authors: {e}"),
                    )
                })?;
            // `fetch_facts_with_authors` may reorder rows relative to
            // `fact_hits`, so key score and per-source rank by id.
            // `fact_hits` arrives ordered by descending ts_rank.
            let (score_by_id, rank_by_id) =
                score_and_rank(fact_hits.iter().map(|h| (h.id, h.score)));
            for (fact, author) in rows {
                let score = score_by_id.get(&fact.id).copied().unwrap_or(0.0);
                let source_rank = rank_by_id.get(&fact.id).copied().unwrap_or(0);
                let author_ref = PublicAuthorRef {
                    agent_name: author.agent_name,
                    model: author.model,
                    repo: author.repo,
                };
                scored.push((
                    score,
                    source_rank,
                    projection::project_fact(&fact, author_ref),
                ));
            }
        }
        if want_event && !event_hits.is_empty() {
            let ids: Vec<uuid::Uuid> = event_hits.iter().map(|h| h.id).collect();
            let rows = state
                .store
                .postgres
                .fetch_events_with_authors(&ids)
                .await
                .map_err(|e| {
                    envelope(
                        errors::INTERNAL_ERROR,
                        format!("fetch_events_with_authors: {e}"),
                    )
                })?;
            let (score_by_id, rank_by_id) =
                score_and_rank(event_hits.iter().map(|h| (h.id, h.score)));
            for (event, author) in rows {
                let score = score_by_id.get(&event.id).copied().unwrap_or(0.0);
                let source_rank = rank_by_id.get(&event.id).copied().unwrap_or(0);
                let author_ref = PublicAuthorRef {
                    agent_name: author.agent_name,
                    model: author.model,
                    repo: author.repo,
                };
                scored.push((
                    score,
                    source_rank,
                    projection::project_event(&event, author_ref),
                ));
            }
        }
    }

    // Tag filter (post-projection): keep memories whose tag set
    // contains all requested tags.
    if let Some(want_tags) = args.tags.as_ref().filter(|v| !v.is_empty()) {
        scored.retain(|(_, _, mem)| want_tags.iter().all(|t| mem.tags.iter().any(|m| m == t)));
    }

    // Cross-source merge by descending score. NOTE (sprint 016): scores
    // are not on a shared scale across kinds (knowledge = Qdrant cosine,
    // fact/event = Postgres ts_rank), so this ordering is biased toward
    // knowledge. Surfaced via ScoredMemory.score, not corrected here.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k as usize);

    mcp_metrics::record_search("anonymous", None);

    Ok(scored
        .into_iter()
        .map(|(score, source_rank, memory)| ScoredMemory {
            score,
            source_rank,
            memory,
        })
        .collect())
}

/// Build `(score_by_id, rank_by_id)` from an ordered iterator of
/// `(id, score)` pairs. The rank is the 0-based enumeration position,
/// i.e. the hit's rank within its source's own result list.
fn score_and_rank(
    hits: impl Iterator<Item = (uuid::Uuid, f32)>,
) -> (HashMap<uuid::Uuid, f32>, HashMap<uuid::Uuid, u32>) {
    let mut score_by_id = HashMap::new();
    let mut rank_by_id = HashMap::new();
    for (rank, (id, score)) in hits.enumerate() {
        score_by_id.insert(id, score);
        rank_by_id.insert(id, rank_u32(rank));
    }
    (score_by_id, rank_by_id)
}

/// Saturating `usize -> u32` for a result-list index. A source never
/// returns more than `MAX_TOP_K` hits, so this never actually saturates.
fn rank_u32(rank: usize) -> u32 {
    u32::try_from(rank).unwrap_or(u32::MAX)
}

async fn fetch_authors(
    state: &McpState,
    ids: &[uuid::Uuid],
) -> HashMap<uuid::Uuid, PublicAuthorRef> {
    let mut out = HashMap::with_capacity(ids.len());
    for id in ids {
        if let Ok(Some(a)) = state.store.postgres.get_author_by_id(*id).await {
            out.insert(
                *id,
                PublicAuthorRef {
                    agent_name: a.agent_name,
                    model: a.model,
                    repo: a.repo,
                },
            );
        }
    }
    out
}

fn unknown_author_ref() -> PublicAuthorRef {
    PublicAuthorRef {
        agent_name: "unknown".into(),
        model: None,
        repo: None,
    }
}
