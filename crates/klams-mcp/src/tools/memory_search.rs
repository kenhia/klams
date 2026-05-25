//! `memory_search` MCP tool (sprint 007 T037, US2).
//!
//! Federates Postgres FTS (facts + events) and Qdrant ANN (knowledge)
//! into a single ranked list of [`PublicMemory`] projections. Soft-
//! deleted rows / points are excluded by the underlying store helpers
//! (`fetch_facts_with_authors` filters `deleted_at IS NULL`;
//! `QdrantStore::search_knowledge` filters `is_empty("deleted_at")`).
//!
//! Optional `kinds` filter narrows which backends are queried.
//! Optional `tags` filter is applied in-memory after projection so the
//! same filter works for facts (which carry no tags column) and
//! knowledge (which does). Result count is clamped to `top_k`.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    metrics as mcp_metrics, projection,
    tools::McpState,
};
use klams_types::{MemoryKind, PublicAuthorRef, PublicMemory};
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

/// Execute `memory_search`. Returns the merged ranked memories on
/// success or an MCP error envelope otherwise.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `EMPTY_QUERY`, `INVALID_TOP_K`,
/// `EMBEDDING_UNAVAILABLE`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run(
    state: &McpState,
    args: MemorySearchArgs,
) -> Result<Vec<PublicMemory>, ErrorEnvelope> {
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

    let mut scored: Vec<(f32, PublicMemory)> = Vec::new();

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
            for (item, score) in hits {
                let author_ref = author_map
                    .get(&item.id)
                    .and_then(|aid| authors.get(aid))
                    .cloned()
                    .unwrap_or_else(unknown_author_ref);
                scored.push((score, projection::project_knowledge(&item, author_ref)));
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
            let score_by_id: HashMap<uuid::Uuid, f32> =
                fact_hits.iter().map(|h| (h.id, h.score)).collect();
            for (fact, author) in rows {
                let score = score_by_id.get(&fact.id).copied().unwrap_or(0.0);
                let author_ref = PublicAuthorRef {
                    agent_name: author.agent_name,
                    model: author.model,
                    repo: author.repo,
                };
                scored.push((score, projection::project_fact(&fact, author_ref)));
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
            let score_by_id: HashMap<uuid::Uuid, f32> =
                event_hits.iter().map(|h| (h.id, h.score)).collect();
            for (event, author) in rows {
                let score = score_by_id.get(&event.id).copied().unwrap_or(0.0);
                let author_ref = PublicAuthorRef {
                    agent_name: author.agent_name,
                    model: author.model,
                    repo: author.repo,
                };
                scored.push((score, projection::project_event(&event, author_ref)));
            }
        }
    }

    // Tag filter (post-projection): keep memories whose tag set
    // contains all requested tags.
    if let Some(want_tags) = args.tags.as_ref().filter(|v| !v.is_empty()) {
        scored.retain(|(_, mem)| want_tags.iter().all(|t| mem.tags.iter().any(|m| m == t)));
    }

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k as usize);

    mcp_metrics::record_search("anonymous", None);

    Ok(scored.into_iter().map(|(_, m)| m).collect())
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
