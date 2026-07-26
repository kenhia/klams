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
//! Sprint 024 (#328): the merge is **rank fusion** (RRF via
//! `klams_core::hybrid::fuse`), so knowledge no longer structurally
//! outranks facts/events. `ScoredMemory.score` is the fused, cross-kind
//! comparable value; `raw_score` carries the pre-fusion per-source
//! relevance (cosine / `ts_rank`) for match-quality evals (#332).

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    metrics as mcp_metrics, projection,
    tools::McpState,
};
use klams_store::{RankedRow, Store};
use klams_types::{
    FusionStrategy, MemoryKind, PublicAuthorRef, PublicMemory, RetrievalSource, ScoredMemory,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULT_TOP_K: u32 = 10;
const MAX_TOP_K: u32 = 50;
const MAX_QUERY_LEN: usize = 1024;

/// Below this top-of-list Qdrant cosine score a knowledge result is a
/// weak semantic match — the exact-identifier gap the §2.1 lexical
/// decision (sprint 024) is measuring. Only applied to a knowledge top
/// hit; fact/event `ts_rank` is not on this scale.
///
/// **Sprint 026 (#643) — recalibrated from 0.5, which could never fire.**
/// bge-small cosine on this corpus sits ~0.75–0.96 *even for junk* (a
/// measured content-free fragment scored 0.956), so the old threshold
/// was below the floor of the distribution: the miss log recorded **one**
/// row in two weeks, and that one was a `zero_hit` from an emptied
/// filter. The observed weak/strong boundary is ~0.78–0.82 (#628's
/// paired queries: the correct answer scored 0.785, the wrong-but-
/// confident competitor 0.790). 0.80 sits in that band.
///
/// This is a *calibrated* constant, not a derived one — it is only
/// honest against the current embedding model.
///
/// **Sprint 028 (#655) — recalibrated for Qwen3-Embedding-0.6B.** The
/// bge-small band above no longer exists: measured live on the rebuilt
/// corpus, a nonsense query ("purple elephant sourdough trampoline…")
/// tops out at raw cosine ~0.35, while genuine hits run ~0.55–0.71.
/// 0.80 would have flagged every search as a miss (the inverse of the
/// 026 dead-threshold bug); 0.45 sits above the junk floor with margin
/// and below the observed real-hit range. Re-derive from the
/// search-sample log if the model changes again.
const LOW_SCORE_THRESHOLD: f32 = 0.45;

/// Over-fetch multiplier for the knowledge ANN search (sprint 026, #641).
/// Query-time duplicate collapse discards hits, so fetching exactly
/// `top_k` would return a short page. Measured: the dominant duplicate
/// shape is a cross-host *pair*, so ×2 restores a full page.
const KNOWLEDGE_OVERFETCH: u32 = 2;

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
/// or an MCP error envelope otherwise. Each result is a [`ScoredMemory`]
/// with the fused `score` (RRF, #328), the per-source `source_rank`, and
/// the pre-fusion `raw_score` (#332), so retrieval evals can see both
/// how a hit ranked and how well it matched.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `EMPTY_QUERY`, `INVALID_TOP_K`,
/// `EMBEDDING_UNAVAILABLE`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run(
    state: &McpState,
    args: MemorySearchArgs,
    caller: Option<&str>,
) -> Result<Vec<ScoredMemory>, ErrorEnvelope> {
    // Sprint 020 (WI #63): MCP is where real search traffic flows;
    // feed the same retrieval-latency histogram the REST handlers use.
    let _guard = klams_core::metrics::LatencyGuard::retrieval("search", "mcp");
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
        // Sprint 024 (#329): route the retrieval sources through the
        // `Store` trait rather than reaching into `.embedder` / `.qdrant`
        // / `.postgres`, so a third source (025 lexical) is added at the
        // trait + fusion seam, not wired into this tool concretely.
        // Sprint 027 (#629): classify rather than assuming transience.
        // A query long enough to exceed the model's ceiling is a
        // permanent `PAYLOAD_TOO_LARGE`, not an outage.
        let embedding = state
            .store
            .embed_query(&query)
            .await
            .map_err(|e| crate::errors::from_store_error("embed_query", &e))?;
        // Sprint 026 (#641): over-fetch so the page is still full after
        // query-time duplicate collapse. ~44% of the corpus is duplicate
        // content (the same chunk stored once per host), so a top_k fetch
        // routinely collapsed to half a page. ×2 covers the measured
        // cross-host pair case; a chunk on three hosts can still shrink
        // the page, which is the accepted top-k-scope tradeoff (#641).
        let fetch_k = top_k.saturating_mul(KNOWLEDGE_OVERFETCH);
        let hits = state
            .store
            .search_knowledge(embedding, fetch_k)
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
                let author_ref = PublicAuthorRef::from_record(&author);
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
                let author_ref = PublicAuthorRef::from_record(&author);
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

    // Query-time duplicate collapse (sprint 026, #641) — BEFORE fusion,
    // so freed ranks compact and the released slots fill with new
    // content instead of leaving holes. Keys on content only (Ken's
    // ruling): the same text on kai and kubs0 is one logical result.
    let before_collapse = scored.len();
    collapse_duplicates_in_place(&mut scored);
    let duplicates_collapsed = before_collapse - scored.len();
    // Collapse removes entries, so the survivors' `source_rank`s are now
    // holed — a caller asking for 20 could see ranks 0 and 25, which
    // exposes the over-fetch multiplier (an implementation detail) as a
    // gap the caller cannot interpret. `source_rank` is contractually
    // "the hit's rank within its own source's result list", and the list
    // the caller receives is the collapsed one, so re-number against it.
    renumber_source_ranks(&mut scored);

    // Snapshot the RAW per-source scores by id before rank-fusion
    // rescoring overwrites them: the miss-log low-score signal is about a
    // weak Qdrant cosine (not the fused RRF value), and the eval surface
    // (#332) exposes raw match quality as ScoredMemory.raw_score.
    let hit_count = scored.len();
    let raw_top = scored
        .iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(s, _, m)| (*s, m.kind()));
    let raw_by_id: HashMap<uuid::Uuid, f32> = scored.iter().map(|(s, _, m)| (m.id, *s)).collect();

    // Cross-source rank fusion (sprint 024 #328). Each source is ranked
    // best-first within itself but on an incomparable score scale
    // (Qdrant cosine vs Postgres ts_rank), so a raw score sort
    // structurally favoured knowledge. Fuse by RRF via
    // `klams_core::hybrid::fuse` — the same ranking `/memory/context`
    // uses — which keys on id + within-source rank, so equal
    // within-source ranks land at comparable positions regardless of
    // kind. Reorder the projections and set `score` to the fused value.
    fuse_in_place(&mut scored, fusion_strategy(state));
    scored.truncate(top_k as usize);

    // Miss log (sprint 021, #317): a search that returned nothing, or
    // only a weak knowledge match, is the "what did an agent want and
    // not get" signal that drives chunking fixes and the lexical-search
    // decision. Fire-and-forget so it never adds latency to a live search.
    if let Some(reason) = classify_miss(hit_count, raw_top) {
        klams_core::metrics::incr_search_miss(reason);
        let miss = klams_store::SearchMiss {
            query: query.clone(),
            caller: caller_label(caller).to_string(),
            reason: reason.to_string(),
            top_score: raw_top.map(|(s, _)| s),
            hit_count: i32::try_from(hit_count).unwrap_or(i32::MAX),
            kinds: kinds_label(&kinds),
        };
        let store = state.store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.postgres.insert_search_miss(&miss).await {
                tracing::debug!(%e, "insert_search_miss failed (miss log best-effort)");
            }
        });
    }

    // Search-sample log (sprint 026, #643): record EVERY search, not
    // just the ones classified as misses. klams had no record of what
    // agents actually ask it — which is why every eval query to date was
    // invented rather than observed, and why the miss threshold above
    // could only be calibrated from #628's handful of data points.
    // Fire-and-forget, same as the miss log.
    {
        let sample = klams_store::SearchSample {
            query: query.clone(),
            caller: caller_label(caller).to_string(),
            top_raw_score: raw_top.map(|(s, _)| s),
            top_kind: raw_top.map(|(_, k)| kind_label(k).to_string()),
            hit_count: i32::try_from(hit_count).unwrap_or(i32::MAX),
            kinds: kinds_label(&kinds),
            duplicates_collapsed: i32::try_from(duplicates_collapsed).unwrap_or(i32::MAX),
        };
        let store = state.store.clone();
        tokio::spawn(async move {
            if let Err(e) = store.postgres.insert_search_sample(&sample).await {
                tracing::debug!(%e, "insert_search_sample failed (sample log best-effort)");
            }
        });
    }

    // Sprint 026 (#643): attribute the caller. This was hardcoded
    // `"anonymous"` since sprint 020, so the per-agent search counter had
    // exactly one label value and answered no question it was added to
    // answer — while the caller's agent name was sitting right there in
    // the argument list.
    mcp_metrics::record_search(caller_label(caller), None);

    Ok(scored
        .into_iter()
        .map(|(score, source_rank, memory)| ScoredMemory {
            score,
            source_rank,
            raw_score: raw_by_id.get(&memory.id).copied(),
            memory,
        })
        .collect())
}

/// Collapse duplicate knowledge hits in place (sprint 026, #641).
///
/// `scored` is best-first within each source, so the surviving copy is
/// the best-ranked one. Fact and event entries carry no `content_hash`
/// and pass through untouched. Each survivor is annotated with the
/// copies it absorbed, so nothing is lost — a caller that wants the copy
/// on a particular host can address it by id.
fn collapse_duplicates_in_place(scored: &mut Vec<(f32, u32, PublicMemory)>) {
    let collapsed = klams_core::dedupe::collapse_duplicates(
        std::mem::take(scored),
        |(_, _, mem)| mem.content.content_hash().map(str::to_string),
        |(_, _, mem)| {
            let (host, file) = match &mem.content {
                klams_types::PublicMemoryContent::Knowledge {
                    host, source_path, ..
                } => (host.clone(), source_path.clone()),
                _ => (None, None),
            };
            klams_types::KnowledgeCopy {
                id: mem.id,
                host,
                file,
            }
        },
    );
    *scored = collapsed
        .into_iter()
        .map(|((score, rank, mut mem), copies)| {
            if !copies.is_empty() {
                mem.content.set_copies(copies);
            }
            (score, rank, mem)
        })
        .collect();
}

/// Re-number `source_rank` per kind, 0-based and contiguous, preserving
/// the current order (sprint 026, #641).
///
/// Each kind's entries are still in that source's best-first order — the
/// collapse only removed entries, it never reordered them — so counting
/// per kind restores the sprint-017 invariant (`source_rank`s of a
/// single-source result are 0-based and contiguous, and a lower rank
/// carries a higher-or-equal score) without re-sorting anything.
fn renumber_source_ranks(scored: &mut [(f32, u32, PublicMemory)]) {
    let mut next: [u32; 3] = [0; 3];
    for (_, rank, mem) in scored.iter_mut() {
        let idx = match mem.kind() {
            MemoryKind::Knowledge => 0usize,
            MemoryKind::Fact => 1,
            MemoryKind::Event => 2,
        };
        *rank = next[idx];
        next[idx] += 1;
    }
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
            out.insert(*id, PublicAuthorRef::from_record(&a));
        }
    }
    out
}

fn unknown_author_ref() -> PublicAuthorRef {
    PublicAuthorRef::unknown()
}

/// Classify a completed search for the miss log (sprint 021 #317).
/// `top` is the score + kind of the highest-ranked surviving hit.
/// Returns `Some(reason)` when the result is a miss, else `None`.
///
/// - **`zero_hit`**: nothing came back — unambiguous, any kind.
/// - **`low_score`**: the top hit is knowledge with a cosine score below
///   [`LOW_SCORE_THRESHOLD`] — a weak semantic match, the signal for
///   the exact-identifier gap. Only knowledge is judged: fact/event
///   `ts_rank` isn't on a comparable scale until sprint 023.
fn classify_miss(hit_count: usize, top: Option<(f32, MemoryKind)>) -> Option<&'static str> {
    if hit_count == 0 {
        return Some("zero_hit");
    }
    if let Some((score, MemoryKind::Knowledge)) = top {
        if score < LOW_SCORE_THRESHOLD {
            return Some("low_score");
        }
    }
    None
}

/// Fusion strategy for cross-source ranking — the `[retrieval] fusion`
/// config, plumbed onto [`McpState`] by `klams-service` (sprint 024
/// #328/#330). Defaults to RRF(k=60) for test harnesses that don't set
/// it.
fn fusion_strategy(state: &McpState) -> FusionStrategy {
    state.fusion
}

/// Rank-fuse `scored` in place (sprint 024 #328): partition into
/// per-kind best-first lists, RRF-fuse via `klams_core::hybrid::fuse`,
/// then reorder the entries by the fused ranking and set each `score` to
/// its fused value. RRF keys on id + within-source rank, so equal
/// within-source ranks land at comparable positions regardless of kind
/// — knowledge no longer structurally outranks facts/events.
fn fuse_in_place(scored: &mut [(f32, u32, PublicMemory)], strategy: FusionStrategy) {
    let mut by_kind: [Vec<RankedRow>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (_, _, mem) in scored.iter() {
        let (idx, source) = match mem.kind() {
            MemoryKind::Knowledge => (0usize, RetrievalSource::Vector),
            MemoryKind::Fact => (1, RetrievalSource::Fts),
            MemoryKind::Event => (2, RetrievalSource::Fts),
        };
        by_kind[idx].push(RankedRow {
            source,
            id: mem.id,
            score: 0.0,
            payload: serde_json::Value::Null,
        });
    }
    let fused = klams_core::hybrid::fuse(by_kind.into_iter().collect(), strategy);
    let order: HashMap<uuid::Uuid, (usize, f32)> = fused
        .iter()
        .enumerate()
        .map(|(pos, r)| (r.id, (pos, r.score)))
        .collect();
    for entry in scored.iter_mut() {
        if let Some(&(_, s)) = order.get(&entry.2.id) {
            entry.0 = s;
        }
    }
    scored.sort_by_key(|(_, _, m)| order.get(&m.id).map_or(usize::MAX, |&(p, _)| p));
}

/// Label for the calling agent (sprint 026, #643). One helper so the
/// miss log, the sample log, and the metric counter can never disagree
/// about who ran a search — they used three different answers before
/// (`"unknown"`, nothing, and the literal `"anonymous"`).
fn caller_label(caller: Option<&str>) -> &str {
    match caller {
        Some(c) if !c.trim().is_empty() => c,
        _ => "unknown",
    }
}

/// Single kind label, for the sample log's `top_kind` column.
fn kind_label(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Fact => "fact",
        MemoryKind::Knowledge => "knowledge",
        MemoryKind::Event => "event",
    }
}

/// Comma-joined kind labels for the miss-log `kinds` column.
fn kinds_label(kinds: &[MemoryKind]) -> String {
    kinds
        .iter()
        .copied()
        .map(kind_label)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_hits_is_a_miss() {
        assert_eq!(classify_miss(0, None), Some("zero_hit"));
    }

    #[test]
    fn weak_knowledge_top_hit_is_low_score() {
        assert_eq!(
            classify_miss(3, Some((0.42, MemoryKind::Knowledge))),
            Some("low_score")
        );
    }

    #[test]
    fn strong_knowledge_top_hit_is_not_a_miss() {
        assert_eq!(classify_miss(3, Some((0.81, MemoryKind::Knowledge))), None);
    }

    // ---- Sprint 026 (#643) introduced these to pin the threshold to
    // the live model's score distribution, precisely so a model swap
    // that shifts the distribution fails here loudly. Sprint 028's swap
    // (bge-small → Qwen3-Embedding-0.6B) did exactly that: these now
    // pin the Qwen3 numbers measured on the rebuilt corpus (junk floor
    // ~0.35, genuine hits ~0.55–0.71).

    #[test]
    fn a_junk_floor_score_fires_the_miss_log() {
        // Measured live: a nonsense query tops out ~0.35 raw cosine on
        // Qwen3. A score at the junk floor must register as a miss.
        assert_eq!(
            classify_miss(5, Some((0.35, MemoryKind::Knowledge))),
            Some("low_score"),
            "a threshold that doesn't fire at the junk floor is dead again"
        );
    }

    #[test]
    fn a_strong_match_still_does_not_fire_the_miss_log() {
        // The overcorrection guard: measured genuine hits run
        // ~0.55–0.71 on Qwen3. If those logged as misses the instrument
        // would be just as useless in the other direction. (This is the
        // test that catches a stale threshold after a model swap: 0.80
        // would flag every one of these.)
        assert_eq!(classify_miss(5, Some((0.71, MemoryKind::Knowledge))), None);
        assert_eq!(classify_miss(5, Some((0.55, MemoryKind::Knowledge))), None);
    }

    #[test]
    fn a_borderline_weak_retrieval_registers_as_weak() {
        // Just under the boundary: "right answer, weak retrieval" is
        // precisely the signal the lexical-gap decision (#333) needs.
        assert_eq!(
            classify_miss(5, Some((0.44, MemoryKind::Knowledge))),
            Some("low_score")
        );
    }

    #[test]
    fn weak_fact_top_hit_is_not_judged() {
        // fact/event ts_rank is not on the cosine scale — don't flag it
        // as low_score (would flood the log). Only knowledge is judged.
        assert_eq!(classify_miss(3, Some((0.01, MemoryKind::Fact))), None);
    }

    // ---- Sprint 026 (#643): caller attribution. `record_search` was
    // hardcoded to "anonymous" since sprint 020, so the per-agent search
    // counter had one label value and answered nothing.

    #[test]
    fn a_known_caller_is_attributed_by_name() {
        assert_eq!(caller_label(Some("claude")), "claude");
    }

    #[test]
    fn an_absent_or_blank_caller_falls_back_to_unknown() {
        // Never "anonymous" — that string was the bug, and a distinct
        // fallback keeps "we don't know" separable from an agent that
        // actually calls itself something.
        assert_eq!(caller_label(None), "unknown");
        assert_eq!(caller_label(Some("   ")), "unknown");
        assert_eq!(caller_label(Some("")), "unknown");
    }

    #[test]
    fn kinds_label_joins_all_three() {
        assert_eq!(
            kinds_label(&[MemoryKind::Fact, MemoryKind::Knowledge, MemoryKind::Event]),
            "fact,knowledge,event"
        );
    }

    // ---- Sprint 024 (#331): hermetic merge-invariant tests over
    // realistic cross-scale magnitudes (Qdrant cosine ~0.8 vs Postgres
    // ts_rank ~0.05). RRF ignores magnitude and fuses by within-source
    // rank, so knowledge no longer structurally outranks facts. No DB.

    use klams_types::{PublicAuthorRef, PublicMemoryContent};

    fn mem(kind: MemoryKind, tag: &str) -> PublicMemory {
        let content = match kind {
            MemoryKind::Knowledge => PublicMemoryContent::Knowledge {
                text: tag.into(),
                source_path: None,
                repo: None,
                host: None,
                content_hash: None,
                heading_path: None,
                language: None,
                chunk_index: None,
                copies: Vec::new(),
            },
            MemoryKind::Fact => PublicMemoryContent::Fact {
                fact_type: "EnvFact".into(),
                payload: serde_json::json!({ "k": tag }),
            },
            MemoryKind::Event => PublicMemoryContent::Event {
                category: "Service".into(),
                payload: serde_json::json!({ "k": tag }),
                task_id: None,
            },
        };
        PublicMemory {
            id: uuid::Uuid::now_v7(),
            content,
            tags: vec![],
            author: PublicAuthorRef {
                id: None,
                agent_name: "t".into(),
                model: None,
                repo: None,
            },
            created_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            updated_at: chrono::DateTime::<chrono::Utc>::UNIX_EPOCH,
            deleted_at: None,
            deleted_by_author_id: None,
        }
    }

    #[test]
    fn rrf_lets_a_top_ranked_fact_beat_a_lower_ranked_knowledge_hit() {
        // Per-source order (as `run` builds it): knowledge best-first,
        // then facts best-first. Raw cosine >> ts_rank, so the OLD raw
        // sort put both knowledge hits above both facts. RRF must
        // interleave by rank: fact@rank0 outranks knowledge@rank1.
        let k0 = mem(MemoryKind::Knowledge, "k0");
        let k1 = mem(MemoryKind::Knowledge, "k1");
        let f0 = mem(MemoryKind::Fact, "f0");
        let (k0id, k1id, f0id) = (k0.id, k1.id, f0.id);
        let mut scored = vec![
            (0.91_f32, 0, k0), // knowledge rank 0, high cosine
            (0.86_f32, 1, k1), // knowledge rank 1
            (0.05_f32, 0, f0), // fact rank 0, tiny ts_rank
        ];
        fuse_in_place(&mut scored, FusionStrategy::default_rrf());
        let pos = |id: uuid::Uuid| scored.iter().position(|(_, _, m)| m.id == id).unwrap();
        // The two within-source rank-0 hits (k0, f0) tie for the top and
        // occupy positions {0,1}; knowledge@rank1 (k1) is last.
        assert!(
            pos(k0id) <= 1 && pos(f0id) <= 1,
            "both rank-0 hits must lead"
        );
        assert_eq!(
            pos(k1id),
            2,
            "knowledge@rank1 must sink below the rank-0 hits"
        );
        assert!(
            pos(f0id) < pos(k1id),
            "fact@rank0 must outrank knowledge@rank1 under RRF (the fix)"
        );
    }

    // ---- Sprint 026 (#641): query-time duplicate collapse. The live
    // failure: the same chunk is stored once per host (sprint 023 made
    // ingest dedupe host-scoped so per-host delete works), so a
    // 10-result page was 5 duplicate pairs.

    /// Knowledge hit with an explicit content hash and host.
    fn dup(hash: &str, host: &str) -> PublicMemory {
        let mut m = mem(MemoryKind::Knowledge, hash);
        m.content = PublicMemoryContent::Knowledge {
            text: format!("body of {hash}"),
            source_path: Some(format!("/home/ken/src/{hash}.md")),
            repo: Some("klams".into()),
            host: Some(host.into()),
            content_hash: Some(hash.into()),
            heading_path: None,
            language: None,
            chunk_index: None,
            copies: Vec::new(),
        };
        m
    }

    fn copies_of(mem: &PublicMemory) -> &[klams_types::KnowledgeCopy] {
        match &mem.content {
            PublicMemoryContent::Knowledge { copies, .. } => copies,
            _ => &[],
        }
    }

    #[test]
    fn a_page_of_duplicate_pairs_collapses_to_distinct_results() {
        // The #641 acceptance case, in miniature: 6 hits that are really
        // 3 chunks, each stored on both hosts.
        let mut scored: Vec<(f32, u32, PublicMemory)> = vec![
            (0.91, 0, dup("aaa", "kai")),
            (0.90, 1, dup("aaa", "kubs0")),
            (0.88, 2, dup("bbb", "kai")),
            (0.87, 3, dup("bbb", "kubs0")),
            (0.85, 4, dup("ccc", "kai")),
            (0.84, 5, dup("ccc", "kubs0")),
        ];
        collapse_duplicates_in_place(&mut scored);
        assert_eq!(scored.len(), 3, "half the page was duplicate content");
        let hashes: Vec<_> = scored
            .iter()
            .map(|(_, _, m)| m.content.content_hash().unwrap().to_string())
            .collect();
        assert_eq!(hashes, vec!["aaa", "bbb", "ccc"]);
        assert!(
            scored.iter().all(|(_, _, m)| copies_of(m).len() == 1),
            "each survivor absorbed exactly one duplicate"
        );
    }

    #[test]
    fn the_survivor_carries_the_collapsed_copies_id_host_and_file() {
        // Ken's ruling: the survivor carries the *list of collapsed
        // points* (lossless), not merged metadata.
        let other = dup("aaa", "kubs0");
        let other_id = other.id;
        let mut scored = vec![(0.91, 0, dup("aaa", "kai")), (0.90, 1, other)];
        collapse_duplicates_in_place(&mut scored);
        assert_eq!(scored.len(), 1);
        let copies = copies_of(&scored[0].2);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].id, other_id, "the copy is addressable by id");
        assert_eq!(copies[0].host.as_deref(), Some("kubs0"));
        assert_eq!(copies[0].file.as_deref(), Some("/home/ken/src/aaa.md"));
    }

    #[test]
    fn collapse_keeps_the_best_ranked_copy_and_its_score() {
        // Collapse must never promote a weaker match over a stronger one.
        let mut scored = vec![(0.91, 0, dup("aaa", "kai")), (0.60, 7, dup("aaa", "kubs0"))];
        collapse_duplicates_in_place(&mut scored);
        assert_eq!(scored.len(), 1);
        assert!((scored[0].0 - 0.91).abs() < f32::EPSILON);
        assert_eq!(scored[0].1, 0, "the best copy's source rank is kept");
        assert_eq!(
            scored[0].2.content,
            PublicMemoryContent::Knowledge {
                text: "body of aaa".into(),
                source_path: Some("/home/ken/src/aaa.md".into()),
                repo: Some("klams".into()),
                host: Some("kai".into()),
                content_hash: Some("aaa".into()),
                heading_path: None,
                language: None,
                chunk_index: None,
                copies: copies_of(&scored[0].2).to_vec(),
            }
        );
    }

    #[test]
    fn facts_and_events_are_never_collapsed() {
        // They carry no content_hash. Two of them must not fuse into one
        // just because both keys are absent.
        let mut scored = vec![
            (0.07, 0, mem(MemoryKind::Fact, "f0")),
            (0.06, 1, mem(MemoryKind::Fact, "f1")),
            (0.05, 0, mem(MemoryKind::Event, "e0")),
        ];
        collapse_duplicates_in_place(&mut scored);
        assert_eq!(scored.len(), 3);
    }

    #[test]
    fn a_knowledge_hit_without_a_hash_is_not_collapsed_into_another() {
        // Pre-022 points and hermetic fixtures have no content_hash.
        let mut scored = vec![
            (0.91, 0, mem(MemoryKind::Knowledge, "k0")),
            (0.90, 1, mem(MemoryKind::Knowledge, "k1")),
        ];
        collapse_duplicates_in_place(&mut scored);
        assert_eq!(scored.len(), 2);
    }

    #[test]
    fn source_ranks_are_renumbered_contiguously_after_a_collapse() {
        // Caught by the docker-gated integration suite, which CI does not
        // run on branches (#646). Survivors kept their PRE-collapse ranks,
        // so a caller saw e.g. [0, 25] — the ×2 over-fetch leaking out as
        // an uninterpretable gap, and a break of the sprint-017 invariant.
        let mut scored: Vec<(f32, u32, PublicMemory)> = vec![
            (0.91, 0, dup("aaa", "kai")),
            (0.90, 1, dup("aaa", "kubs0")),
            (0.89, 2, dup("aaa", "cleo")),
            (0.88, 3, dup("bbb", "kai")),
        ];
        collapse_duplicates_in_place(&mut scored);
        renumber_source_ranks(&mut scored);
        assert_eq!(
            scored.iter().map(|(_, r, _)| *r).collect::<Vec<_>>(),
            vec![0, 1],
            "ranks must be 0-based and contiguous over the collapsed list"
        );
    }

    #[test]
    fn renumbering_counts_each_kind_independently() {
        // `source_rank` is per-source, so knowledge and facts each start
        // at 0 — interleaving them into one counter would make a fact at
        // rank 0 look like it outranked the knowledge hit above it.
        let mut scored = vec![
            (0.91, 9, mem(MemoryKind::Knowledge, "k0")),
            (0.07, 4, mem(MemoryKind::Fact, "f0")),
            (0.90, 7, mem(MemoryKind::Knowledge, "k1")),
            (0.06, 2, mem(MemoryKind::Fact, "f1")),
            (0.05, 8, mem(MemoryKind::Event, "e0")),
        ];
        renumber_source_ranks(&mut scored);
        assert_eq!(
            scored.iter().map(|(_, r, _)| *r).collect::<Vec<_>>(),
            vec![0, 0, 1, 1, 0]
        );
    }

    #[test]
    fn renumbering_preserves_order_and_scores() {
        let mut scored = vec![
            (0.91, 5, mem(MemoryKind::Knowledge, "k0")),
            (0.80, 9, mem(MemoryKind::Knowledge, "k1")),
        ];
        let ids: Vec<_> = scored.iter().map(|(_, _, m)| m.id).collect();
        renumber_source_ranks(&mut scored);
        assert_eq!(scored.iter().map(|(_, _, m)| m.id).collect::<Vec<_>>(), ids);
        // The sprint-017 pairing still holds: lower rank, higher score.
        assert!(scored[0].0 >= scored[1].0);
    }

    #[test]
    fn collapse_leaves_no_duplicate_hash_in_the_page() {
        // The invariant the eval suite (#643) asserts, stated here so a
        // regression fails in unit tests too.
        let mut scored: Vec<(f32, u32, PublicMemory)> = vec![
            (0.91, 0, dup("aaa", "kai")),
            (0.90, 1, dup("bbb", "kai")),
            (0.89, 2, dup("aaa", "kubs0")),
            (0.88, 3, dup("aaa", "cleo")),
            (0.87, 4, dup("bbb", "kubs0")),
        ];
        collapse_duplicates_in_place(&mut scored);
        let mut seen = std::collections::HashSet::new();
        for (_, _, m) in &scored {
            let h = m.content.content_hash().unwrap().to_string();
            assert!(seen.insert(h), "duplicate content_hash survived the page");
        }
        assert_eq!(copies_of(&scored[0].2).len(), 2, "aaa absorbed two copies");
    }

    #[test]
    fn rrf_output_is_sorted_by_fused_score_descending() {
        let mut scored = vec![
            (0.9_f32, 0, mem(MemoryKind::Knowledge, "k0")),
            (0.8_f32, 1, mem(MemoryKind::Knowledge, "k1")),
            (0.07_f32, 0, mem(MemoryKind::Fact, "f0")),
            (0.03_f32, 1, mem(MemoryKind::Event, "e0")),
        ];
        fuse_in_place(&mut scored, FusionStrategy::default_rrf());
        assert!(
            scored.windows(2).all(|w| w[0].0 >= w[1].0),
            "fused scores must be descending: {:?}",
            scored.iter().map(|(s, _, _)| *s).collect::<Vec<_>>()
        );
        // Every score is now the RRF value (<= 1/(k+1)), never a raw cosine.
        assert!(scored
            .iter()
            .all(|(s, _, _)| *s <= 1.0 / 61.0 + f32::EPSILON));
    }
}
