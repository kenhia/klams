//! The one retrieval pipeline (sprint 036, #730).
//!
//! Lifted from `klams-mcp`'s `memory_search` tool, which was the only
//! surface running the *real* pipeline — curated stratum, three-tier
//! provenance weights, boost gate, duplicate collapse, second-stage
//! rerank, weighted RRF, miss + sample logs — while REST
//! `/memory/search` ran a divergent adapter path (no curated stratum,
//! no rerank, author-blind two-tier weights, hardcoded fusion) and
//! `memory_related` was bare ANN. Both surfaces now call [`search`] /
//! [`related`] here; the tool and handler are validation + envelope
//! shells that map [`RetrievalError`] onto their own wire contracts.
//!
//! Federates Postgres FTS (facts + events) and Qdrant ANN (knowledge)
//! into a single ranked list of [`ScoredMemory`] envelopes (sprint 016
//! — each wraps a [`PublicMemory`] projection with its relevance
//! `score` and per-source rank). Soft-deleted rows / points are
//! excluded by the underlying store helpers (`fetch_facts_with_authors`
//! filters `deleted_at IS NULL`; `QdrantStore::search_knowledge`
//! filters `is_empty("deleted_at")`) — which also excludes superseded
//! knowledge, since `mark_superseded` is a soft-delete plus a
//! `superseded_by` pointer.
//!
//! Sprint 024 (#328): the merge is **rank fusion** (RRF via
//! [`crate::hybrid::fuse`]), so knowledge no longer structurally
//! outranks facts/events. `ScoredMemory.score` is the fused, cross-kind
//! comparable value; `raw_score` carries the pre-fusion per-source
//! relevance (cosine / `ts_rank`) for match-quality evals (#332).

use crate::projection;
use crate::provenance::{volatility_demotion, ProvenanceTier};
use klams_store::{RankedRow, Store, StoreError, TeiReranker};
use klams_types::{
    Event, Fact, FusionStrategy, KnowledgeItem, MemoryKind, PublicAuthorRef, PublicMemory,
    RetrievalFilters, RetrievalSource, ScoredMemory,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const DEFAULT_TOP_K: u32 = 10;
pub const MAX_TOP_K: u32 = 50;
pub const MAX_QUERY_LEN: usize = 1024;
pub const DEFAULT_RERANK_WINDOW: usize = 50;

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
/// `top_k` would return a short page.
///
/// History: calibrated when the dominant duplicate shape was a
/// cross-host *pair* (44% of the pre-028 corpus). The 028 ingest-time
/// dedupe (#642) collapsed those into single points with a `machines[]`
/// list, so the residue this now covers is the check-then-enqueue
/// publish race (review F-4.3) plus same-content copies the race let
/// through — re-measured 2026-07-28 (033 retrospective): 71 duplicate
/// groups / 205 points in a 180k corpus, but they cluster on popular
/// content, so 19% of live searches still collapse ≥1 hit. ×2 remains
/// the right cheap insurance; the constant outlived its original
/// rationale, not its usefulness.
const KNOWLEDGE_OVERFETCH: u32 = 2;

/// FTS over-fetch multiplier when request filters are active (sprint
/// 036, #730). Filtering discards rows post-fetch, so a plain `top_k`
/// fetch would return a short page for any selective filter — the same
/// full-page property the old REST adapter bought with its blanket ×3.
/// Unfiltered searches skip it: nothing is discarded, so nothing needs
/// over-fetching.
const FILTER_OVERFETCH: u32 = 3;

/// Runtime configuration for the shared pipeline — the `[retrieval]`
/// config, plumbed here by `klams-service` for both surfaces.
#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    /// Cross-source rank-fusion strategy (sprint 024 #328/#330).
    pub fusion: FusionStrategy,
    /// Optional second-stage cross-encoder (sprint 030, #685).
    /// `None` = the stage is off.
    pub reranker: Option<Arc<TeiReranker>>,
    /// Max candidates per rerank call (`[retrieval] rerank_window`).
    pub rerank_window: usize,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            fusion: FusionStrategy::default_rrf(),
            reranker: None,
            rerank_window: DEFAULT_RERANK_WINDOW,
        }
    }
}

/// What to do when a retrieval source fails mid-search (sprint 036,
/// #730). The two surfaces carry different contracts — MCP hard-fails
/// with a typed, retry-classified error; REST serves what it can with
/// `degraded: true` — and the shared core expresses both rather than
/// picking one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTolerance {
    /// Any source failure aborts the search ([`search`] returns `Err`).
    /// The MCP `memory_search` contract.
    HardFail,
    /// A failed source contributes nothing; the search continues and
    /// the outcome is flagged [`SearchOutcome::degraded`]. The REST
    /// `/memory/search` contract.
    Degrade,
}

/// Validated-on-entry search request for [`search`].
#[derive(Debug, Clone)]
pub struct SearchParams {
    pub query: String,
    /// `None` = all three kinds.
    pub kinds: Option<Vec<MemoryKind>>,
    /// All listed tags must be present on a hit (MCP contract).
    pub tags: Option<Vec<String>>,
    /// REST filter contract (033, #692). MCP passes the default.
    pub filters: RetrievalFilters,
    /// `None` = [`DEFAULT_TOP_K`]. Values outside `1..=MAX_TOP_K` are
    /// an [`RetrievalError::InvalidTopK`]; clamp before calling if the
    /// surface's contract is clamp-not-reject.
    pub top_k: Option<u32>,
}

impl SearchParams {
    /// Minimal params for a plain query — all kinds, no tags, no
    /// filters, default `top_k`.
    #[must_use]
    pub fn query(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            kinds: None,
            tags: None,
            filters: RetrievalFilters::default(),
            top_k: None,
        }
    }
}

/// A completed search: the fused hits plus whether any source was
/// skipped under [`SourceTolerance::Degrade`].
#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub hits: Vec<ScoredMemory>,
    pub degraded: bool,
}

/// Pipeline failure, neutral to both wire contracts (sprint 036, #730).
/// The MCP shell maps these onto `ErrorEnvelope` codes; the REST
/// handler onto `ApiError`. Variants preserve the underlying
/// [`StoreError`] so the 027 transient/permanent taxonomy survives the
/// mapping.
#[derive(Debug, thiserror::Error)]
pub enum RetrievalError {
    #[error("query must be non-empty")]
    EmptyQuery,
    #[error("query exceeds {max} characters")]
    QueryTooLong { max: usize },
    #[error("top_k must be 1..={max}")]
    InvalidTopK { max: u32 },
    /// The query embedding failed. Kept distinct from [`Self::Backend`]
    /// because the embedder's error taxonomy (transient outage vs
    /// permanent `PayloadTooLarge`) has its own wire codes on the MCP
    /// surface (sprint 027, #629).
    #[error("embed_query: {0}")]
    Embed(StoreError),
    #[error("{context}: {source}")]
    Backend {
        context: &'static str,
        source: StoreError,
    },
    #[error("not found: {0}")]
    NotFound(String),
}

impl RetrievalError {
    fn backend(context: &'static str) -> impl FnOnce(StoreError) -> Self {
        move |source| Self::Backend { context, source }
    }
}

/// Execute a memory search over the shared pipeline. Returns the merged
/// ranked hits on success. Each result is a [`ScoredMemory`] with the
/// fused `score` (RRF, #328), the per-source `source_rank`, and the
/// pre-fusion `raw_score` (#332), so retrieval evals can see both how a
/// hit ranked and how well it matched.
///
/// `caller` labels the miss/sample logs and metrics; `transport` labels
/// the latency histogram (`"mcp"` / `"rest"`).
///
/// # Errors
/// Validation errors always; source failures only under
/// [`SourceTolerance::HardFail`] (under `Degrade` they set
/// [`SearchOutcome::degraded`] instead).
#[allow(clippy::too_many_lines)]
pub async fn search<S: Store>(
    store: &Arc<S>,
    params: SearchParams,
    config: &RetrievalConfig,
    tolerance: SourceTolerance,
    caller: Option<&str>,
    transport: &'static str,
) -> Result<SearchOutcome, RetrievalError> {
    // Sprint 020 (WI #63): feed the same retrieval-latency histogram
    // from every surface the pipeline serves.
    let _guard = crate::metrics::LatencyGuard::retrieval("search", transport);
    let query = params.query.trim().to_string();
    if query.is_empty() {
        return Err(RetrievalError::EmptyQuery);
    }
    if query.len() > MAX_QUERY_LEN {
        return Err(RetrievalError::QueryTooLong { max: MAX_QUERY_LEN });
    }
    let top_k = params.top_k.unwrap_or(DEFAULT_TOP_K);
    if top_k == 0 || top_k > MAX_TOP_K {
        return Err(RetrievalError::InvalidTopK { max: MAX_TOP_K });
    }

    let kinds: Vec<MemoryKind> = params
        .kinds
        .clone()
        .unwrap_or_else(|| vec![MemoryKind::Fact, MemoryKind::Knowledge, MemoryKind::Event]);
    let want_fact = kinds.contains(&MemoryKind::Fact);
    let want_knowledge = kinds.contains(&MemoryKind::Knowledge);
    let want_event = kinds.contains(&MemoryKind::Event);
    let filters = &params.filters;

    // Each entry: (score, source_rank, memory). `source_rank` is the
    // hit's 0-based position within its own source's result list,
    // captured before the cross-source merge below.
    let mut scored: Vec<(f32, u32, PublicMemory)> = Vec::new();

    // Sprint 029 (#644): per-hit fusion weights (provenance tier ×
    // declared-volatility age demotion) and the curated stratum's own
    // rank order, fed to `fuse_in_place` as a 4th RRF source.
    let mut weights: HashMap<uuid::Uuid, f32> = HashMap::new();
    let mut curated_order: Vec<uuid::Uuid> = Vec::new();
    let mut lexical_order: Vec<uuid::Uuid> = Vec::new();

    let mut degraded = false;

    // ---------- knowledge via Qdrant ANN ----------
    if want_knowledge {
        match knowledge_candidates(store, &query, top_k, filters).await {
            Ok(Some(k)) => {
                weights = k.weights;
                curated_order = k.curated_order;
                lexical_order = k.lexical_order;
                scored = k.scored;
            }
            Ok(None) => {}
            Err(e) => match tolerance {
                SourceTolerance::HardFail => return Err(e),
                SourceTolerance::Degrade => {
                    tracing::warn!(error = %e, "knowledge source failed; degrading");
                    degraded = true;
                }
            },
        }
    }

    // ---------- facts + events via Postgres FTS ----------
    if want_fact || want_event {
        // Sprint 036 (#730): each of facts and events gets its own
        // `top_k` budget. The old REST adapter truncated one shared
        // budget BEFORE the fact/event split, so an event-heavy query
        // could return zero facts; MCP always capped them separately,
        // and that is the behavior the unified pipeline keeps.
        let fetch_k = if filters_active(filters) {
            top_k.saturating_mul(FILTER_OVERFETCH)
        } else {
            top_k
        };
        match fts_candidates(
            store, &query, fetch_k, top_k, want_fact, want_event, filters,
        )
        .await
        {
            Ok(mut rows) => scored.append(&mut rows),
            Err(e) => match tolerance {
                SourceTolerance::HardFail => return Err(e),
                SourceTolerance::Degrade => {
                    tracing::warn!(error = %e, "fts source failed; degrading");
                    degraded = true;
                }
            },
        }
    }

    // Tag filter (post-projection): keep memories whose tag set
    // contains all requested tags.
    if let Some(want_tags) = params.tags.as_ref().filter(|v| !v.is_empty()) {
        scored.retain(|(_, _, mem)| want_tags.iter().all(|t| mem.tags.iter().any(|m| m == t)));
    }

    // Query-time duplicate collapse (sprint 026, #641) — BEFORE fusion,
    // so freed ranks compact and the released slots fill with new
    // content instead of leaving holes. Keys on content only (Ken's
    // ruling): the same text on kai and kubs0 is one logical result.
    let before_collapse = scored.len();
    collapse_duplicates_in_place(&mut scored);
    let duplicates_collapsed = before_collapse - scored.len();
    // The tag filter and the collapse can both remove entries the
    // curated stratum or lexical list named; a fused rank for an id with
    // no surviving projection would push real results down the page for
    // a ghost.
    {
        let live: HashSet<uuid::Uuid> = scored.iter().map(|(_, _, m)| m.id).collect();
        curated_order.retain(|id| live.contains(id));
        lexical_order.retain(|id| live.contains(id));
    }
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

    // Second-stage rerank (sprint 030, #685) — AFTER the raw-score
    // snapshot (raw_score stays the cosine; the cross-encoder only
    // reorders), BEFORE fusion, so provenance weights and the curated
    // stratum's boost apply to the RERANKED order. Best-effort: any
    // reranker failure logs and falls through to the un-reranked order.
    if let Some(reranker) = config.reranker.as_ref() {
        rerank_stage(
            reranker,
            &query,
            &mut scored,
            &mut curated_order,
            &mut lexical_order,
            config.rerank_window,
            transport,
        )
        .await;
    }

    // Cross-source rank fusion (sprint 024 #328). Each source is ranked
    // best-first within itself but on an incomparable score scale
    // (Qdrant cosine vs Postgres ts_rank), so a raw score sort
    // structurally favoured knowledge. Fuse by RRF via
    // `crate::hybrid::fuse` — the same ranking `/memory/context` uses —
    // which keys on id + within-source rank, so equal within-source
    // ranks land at comparable positions regardless of kind. Reorder
    // the projections and set `score` to the fused value.
    fuse_in_place(
        &mut scored,
        config.fusion,
        &weights,
        &curated_order,
        &lexical_order,
    );
    scored.truncate(top_k as usize);

    // Miss log (sprint 021, #317): a search that returned nothing, or
    // only a weak knowledge match, is the "what did an agent want and
    // not get" signal that drives chunking fixes and the lexical-search
    // decision. Fire-and-forget so it never adds latency to a live search.
    if let Some(reason) = classify_miss(hit_count, raw_top) {
        crate::metrics::incr_search_miss(reason);
        let miss = klams_store::SearchMiss {
            query: query.clone(),
            caller: caller_label(caller).to_string(),
            reason: reason.to_string(),
            top_score: raw_top.map(|(s, _)| s),
            hit_count: i32::try_from(hit_count).unwrap_or(i32::MAX),
            kinds: kinds_label(&kinds),
        };
        let store = Arc::clone(store);
        tokio::spawn(async move {
            if let Err(e) = store.insert_search_miss(&miss).await {
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
        let store = Arc::clone(store);
        tokio::spawn(async move {
            if let Err(e) = store.insert_search_sample(&sample).await {
                tracing::debug!(%e, "insert_search_sample failed (sample log best-effort)");
            }
        });
    }

    let hits = scored
        .into_iter()
        .map(|(score, source_rank, memory)| ScoredMemory {
            score,
            source_rank,
            raw_score: raw_by_id.get(&memory.id).copied(),
            memory,
        })
        .collect();
    Ok(SearchOutcome { hits, degraded })
}

/// Nearest neighbours of an existing knowledge memory (sprint 007 T038,
/// unified into the shared pipeline in 036, #730). Only knowledge ids
/// are supported — facts and events have no vector representation.
///
/// What the shared pipeline adds over the old bare-ANN implementation:
/// duplicate collapse (a neighbour stored on several hosts is ONE
/// result carrying its copies), exclusion of copies of the *seed's own
/// content* (a duplicate of the seed is not a "related" memory), and
/// the same live-only guarantee search has (superseded / soft-deleted
/// points are filtered by the ANN query itself). The seed may itself be
/// superseded — its vector still resolves, and the neighbours returned
/// are the live memories nearest to it.
///
/// # Errors
/// [`RetrievalError::InvalidTopK`], [`RetrievalError::NotFound`] (nil
/// or unknown id), or [`RetrievalError::Backend`].
pub async fn related<S: Store>(
    store: &Arc<S>,
    id: uuid::Uuid,
    top_k: Option<u32>,
) -> Result<Vec<PublicMemory>, RetrievalError> {
    let top_k = top_k.unwrap_or(5);
    if top_k == 0 || top_k > MAX_TOP_K {
        return Err(RetrievalError::InvalidTopK { max: MAX_TOP_K });
    }
    if id.is_nil() {
        return Err(RetrievalError::NotFound("id is nil".into()));
    }

    let vector = store
        .get_point_vector(id)
        .await
        .map_err(RetrievalError::backend("get_point_vector"))?
        .ok_or_else(|| RetrievalError::NotFound(format!("knowledge point {id} not found")))?;

    // The seed's own content hash, so copies of the seed stored under
    // other ids don't come back as "related" — they are the same
    // memory, not a neighbour. Best-effort: a seed the store can't
    // fetch metadata for just skips this exclusion.
    let seed_hash: Option<String> = store
        .get_knowledge(id)
        .await
        .ok()
        .flatten()
        .map(|item| item.content_hash);

    // Over-fetch: +1 for the seed point itself (similarity 1.0), ×2 for
    // duplicate-collapse headroom (#641), same rationale as search.
    let fetch_k = (top_k + 1).saturating_mul(KNOWLEDGE_OVERFETCH);
    let raw = store
        .search_knowledge(vector, fetch_k)
        .await
        .map_err(RetrievalError::backend("search_knowledge"))?;
    let hits: Vec<(KnowledgeItem, f32)> = raw
        .into_iter()
        .filter(|(it, _)| it.id != id)
        .filter(|(it, _)| match (&seed_hash, &it.content_hash) {
            (Some(seed), hash) => seed != hash,
            (None, _) => true,
        })
        .collect();

    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<uuid::Uuid> = hits.iter().map(|(it, _)| it.id).collect();
    let authors = resolve_knowledge_authors(store, &ids).await;
    let mut scored: Vec<(f32, u32, PublicMemory)> = hits
        .into_iter()
        .enumerate()
        .map(|(rank, (item, score))| {
            let author_ref = authors.author_ref_for(item.id);
            (
                score,
                rank_u32(rank),
                projection::project_knowledge(&item, author_ref),
            )
        })
        .collect();
    collapse_duplicates_in_place(&mut scored);
    scored.truncate(top_k as usize);
    Ok(scored.into_iter().map(|(_, _, mem)| mem).collect())
}

/// Knowledge-arm result: the global + curated candidates, their fusion
/// weights, and the curated stratum's own rank order.
struct KnowledgeCandidates {
    scored: Vec<(f32, u32, PublicMemory)>,
    weights: HashMap<uuid::Uuid, f32>,
    curated_order: Vec<uuid::Uuid>,
    lexical_order: Vec<uuid::Uuid>,
}

/// The ANN + curated-stratum candidate stage. `Ok(None)` = both lists
/// empty.
async fn knowledge_candidates<S: Store>(
    store: &Arc<S>,
    query: &str,
    top_k: u32,
    filters: &RetrievalFilters,
) -> Result<Option<KnowledgeCandidates>, RetrievalError> {
    // Sprint 024 (#329): route the retrieval sources through the
    // `Store` trait rather than reaching into `.embedder` / `.qdrant`
    // / `.postgres`, so a third source is added at the trait + fusion
    // seam, not wired into the pipeline concretely. Sprint 027 (#629):
    // embed errors keep their transient/permanent classification.
    let embedding = store
        .embed_query(query)
        .await
        .map_err(RetrievalError::Embed)?;
    // Sprint 026 (#641): over-fetch so the page is still full after
    // query-time duplicate collapse; sprint 036: filters discard more,
    // so widen the fetch when they are active.
    let overfetch = if filters_active(filters) {
        KNOWLEDGE_OVERFETCH.max(FILTER_OVERFETCH)
    } else {
        KNOWLEDGE_OVERFETCH
    };
    let fetch_k = top_k.saturating_mul(overfetch);
    let hits: Vec<(KnowledgeItem, f32)> = store
        .search_knowledge(embedding.clone(), fetch_k)
        .await
        .map_err(RetrievalError::backend("search_knowledge"))?
        .into_iter()
        .filter(|(item, _)| knowledge_matches_filters(item, filters))
        .take(top_k.saturating_mul(KNOWLEDGE_OVERFETCH) as usize)
        .collect();
    // Sprint 029 (#644): the curated stratum. Agent-authored knowledge
    // is ~100 points in a ~180k corpus, so a badly-phrased query can
    // miss the curated target in ANY global top-k (#628's Query A
    // failure mode). A filtered ANN search over just that stratum
    // always surfaces its best matches; it reuses the query vector and
    // enters fusion as a 4th rank list.
    //
    // The boost threshold is query-relative (`boost_threshold`):
    // stratum membership and the tier weight both require a raw score
    // competitive with the query's best hit. Measured without it,
    // topically-adjacent agent memories (raw 0.60) flooded out a
    // genuine bulk answer (raw 0.75) — a boosted curated hit at any
    // stratum rank outscores every unboosted rank-0 hit, so
    // eligibility, not fusion arithmetic, is where relevance has to
    // hold the line.
    let all_curated: Vec<(KnowledgeItem, f32)> = store
        .search_knowledge_curated(embedding.clone(), top_k)
        .await
        .map_err(RetrievalError::backend("curated search"))?
        .into_iter()
        .filter(|(item, _)| knowledge_matches_filters(item, filters))
        .collect();
    // Sprint 037 (#333): the lexical candidate list — live points whose
    // text contains every query token, cosine-ordered. This is the
    // lexical source the crossroads §2.1 gap called for, in its
    // cheapest adequate form (Qdrant full-text payload index; no BM25
    // engine). Deliberately NOT gated by `boost_threshold`: the
    // motivating hit ("klams gotcha" → the curated MCP gotcha, raw
    // 0.41 vs a 0.49 bulk top) sits below any competitive-score gate —
    // the all-tokens-present match is itself the relevance evidence.
    let lexical_hits: Vec<(KnowledgeItem, f32)> = store
        .search_knowledge_lexical(query, embedding, top_k)
        .await
        .map_err(RetrievalError::backend("lexical search"))?
        .into_iter()
        .filter(|(item, _)| knowledge_matches_filters(item, filters))
        .collect();
    let top_raw = hits
        .iter()
        .chain(all_curated.iter())
        .map(|(_, s)| *s)
        .fold(0.0_f32, f32::max);
    let threshold = crate::provenance::boost_threshold(top_raw);
    let curated_hits: Vec<(KnowledgeItem, f32)> = all_curated
        .into_iter()
        .filter(|(_, score)| *score >= threshold)
        .collect();
    if hits.is_empty() && curated_hits.is_empty() && lexical_hits.is_empty() {
        return Ok(None);
    }

    let ids: Vec<uuid::Uuid> = hits
        .iter()
        .chain(curated_hits.iter())
        .chain(lexical_hits.iter())
        .map(|(it, _)| it.id)
        .collect();
    let authors = resolve_knowledge_authors(store, &ids).await;

    let mut weights: HashMap<uuid::Uuid, f32> = HashMap::new();
    let mut scored: Vec<(f32, u32, PublicMemory)> = Vec::new();

    let in_global: HashSet<uuid::Uuid> = hits.iter().map(|(it, _)| it.id).collect();
    let global_count = hits.len();
    // Qdrant returns hits already ordered by descending similarity, so
    // enumerate() yields the source rank.
    for (rank, (item, score)) in hits.into_iter().enumerate() {
        let author_ref = authors.author_ref_for(item.id);
        weights.insert(
            item.id,
            knowledge_weight(&item, &author_ref.agent_name, score >= threshold),
        );
        scored.push((
            score,
            rank_u32(rank),
            projection::project_knowledge(&item, author_ref),
        ));
    }
    // Stratum hits join the knowledge list. Ones already in the global
    // page keep their entry (the 4th fusion list is what boosts them);
    // stratum-only hits are appended after the global hits, so the
    // knowledge list stays contiguous and best-first per source.
    // Stratum membership already implies `score >= threshold`.
    let mut page = KnowledgePage {
        present: in_global,
        next_rank: global_count,
        authors: &authors,
        weights: &mut weights,
        scored: &mut scored,
    };
    let curated_order = page.append_aux_list(curated_hits, |_| true);
    // Lexical hits follow the same shape as the stratum: already-present
    // ids keep their entry (the 5th fusion list is their lift); lexical-
    // only hits are appended so they exist on the page at all. Their tier
    // weight applies un-boosted unless they independently clear the
    // competitive threshold — the lexical list's rank contribution, not
    // a provenance boost, is what argues for them.
    let lexical_order = page.append_aux_list(lexical_hits, |score| score >= threshold);
    Ok(Some(KnowledgeCandidates {
        scored,
        weights,
        curated_order,
        lexical_order,
    }))
}

/// In-progress knowledge page state shared by the auxiliary rank lists
/// (curated stratum, lexical list) while they join the candidate page.
struct KnowledgePage<'a> {
    present: HashSet<uuid::Uuid>,
    next_rank: usize,
    authors: &'a KnowledgeAuthors,
    weights: &'a mut HashMap<uuid::Uuid, f32>,
    scored: &'a mut Vec<(f32, u32, PublicMemory)>,
}

impl KnowledgePage<'_> {
    /// Record an auxiliary list's rank order and append its hits the
    /// page doesn't already hold. Returns the list's own order for
    /// fusion; `boosted` decides per-hit whether the provenance tier
    /// weight applies at full strength.
    fn append_aux_list(
        &mut self,
        hits: Vec<(KnowledgeItem, f32)>,
        boosted: impl Fn(f32) -> bool,
    ) -> Vec<uuid::Uuid> {
        let mut order = Vec::with_capacity(hits.len());
        for (item, score) in hits {
            order.push(item.id);
            if !self.present.insert(item.id) {
                continue;
            }
            let author_ref = self.authors.author_ref_for(item.id);
            self.weights.insert(
                item.id,
                knowledge_weight(&item, &author_ref.agent_name, boosted(score)),
            );
            self.scored.push((
                score,
                rank_u32(self.next_rank),
                projection::project_knowledge(&item, author_ref),
            ));
            self.next_rank += 1;
        }
        order
    }
}

/// The facts + events FTS candidate stage. `fetch_k` is the per-source
/// DB fetch budget (over-fetched when filters are active); `top_k` caps
/// each kind's surviving rows separately.
async fn fts_candidates<S: Store>(
    store: &Arc<S>,
    query: &str,
    fetch_k: u32,
    top_k: u32,
    want_fact: bool,
    want_event: bool,
    filters: &RetrievalFilters,
) -> Result<Vec<(f32, u32, PublicMemory)>, RetrievalError> {
    let mut out: Vec<(f32, u32, PublicMemory)> = Vec::new();
    let (fact_hits, event_hits) = store
        .search_text(query, fetch_k)
        .await
        .map_err(RetrievalError::backend("search_text"))?;
    if want_fact && !fact_hits.is_empty() {
        let ids: Vec<uuid::Uuid> = fact_hits.iter().map(|h| h.id).collect();
        let rows = store
            .fetch_facts_with_authors(&ids)
            .await
            .map_err(RetrievalError::backend("fetch_facts_with_authors"))?;
        // `fetch_facts_with_authors` may reorder rows relative to
        // `fact_hits`, so key the typed rows by id and walk `fact_hits`
        // (ordered by descending ts_rank) to keep the source order —
        // filtered survivors are then ranked by their surviving
        // position, matching the "rank within the list the caller
        // receives" contract.
        let score_by_id: HashMap<uuid::Uuid, f32> =
            fact_hits.iter().map(|h| (h.id, h.score)).collect();
        let mut by_id: HashMap<uuid::Uuid, (Fact, klams_types::AuthorRecord)> =
            rows.into_iter().map(|(f, a)| (f.id, (f, a))).collect();
        let mut rank = 0usize;
        for hit in &fact_hits {
            let Some((fact, author)) = by_id.remove(&hit.id) else {
                continue;
            };
            if !fact_matches_filters(&fact, filters) {
                continue;
            }
            if rank >= top_k as usize {
                break;
            }
            let hit_score = score_by_id.get(&fact.id).copied().unwrap_or(0.0);
            let author_ref = PublicAuthorRef::from_record(&author);
            out.push((
                hit_score,
                rank_u32(rank),
                projection::project_fact(&fact, author_ref),
            ));
            rank += 1;
        }
    }
    if want_event && !event_hits.is_empty() {
        let ids: Vec<uuid::Uuid> = event_hits.iter().map(|h| h.id).collect();
        let rows = store
            .fetch_events_with_authors(&ids)
            .await
            .map_err(RetrievalError::backend("fetch_events_with_authors"))?;
        let score_by_id: HashMap<uuid::Uuid, f32> =
            event_hits.iter().map(|h| (h.id, h.score)).collect();
        let mut by_id: HashMap<uuid::Uuid, (Event, klams_types::AuthorRecord)> =
            rows.into_iter().map(|(e, a)| (e.id, (e, a))).collect();
        let mut rank = 0usize;
        for hit in &event_hits {
            let Some((event, author)) = by_id.remove(&hit.id) else {
                continue;
            };
            if !event_matches_filters(&event, filters) {
                continue;
            }
            if rank >= top_k as usize {
                break;
            }
            let hit_score = score_by_id.get(&event.id).copied().unwrap_or(0.0);
            let author_ref = PublicAuthorRef::from_record(&author);
            out.push((
                hit_score,
                rank_u32(rank),
                projection::project_event(&event, author_ref),
            ));
            rank += 1;
        }
    }
    Ok(out)
}

/// Any filter set at all? Decides whether the candidate stages pay for
/// over-fetch headroom.
fn filters_active(f: &RetrievalFilters) -> bool {
    f.host.is_some()
        || f.type_.is_some()
        || f.tag.is_some()
        || f.repo.is_some()
        || f.file.is_some()
        || f.source.is_some()
        || f.since.is_some()
        || f.until.is_some()
}

/// Apply [`RetrievalFilters`] to a knowledge candidate on its typed
/// fields (sprint 036, #730 — the adapter matched against payload JSON
/// keys instead; typed matching also fixes two conservative-drop
/// quirks: `since`/`until` now filter on `created_at` rather than
/// dropping every knowledge row, and `source` matches the real trust
/// tier). A `type` filter names a `FactType`, which knowledge does not
/// have — it excludes all knowledge, as before.
fn knowledge_matches_filters(item: &KnowledgeItem, f: &RetrievalFilters) -> bool {
    if f.type_.is_some() {
        return false;
    }
    if let Some(want) = f.host.as_deref() {
        if item.machine.as_deref() != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.repo.as_deref() {
        if item.repo.as_deref() != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.file.as_deref() {
        if item.file.as_deref() != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.tag.as_deref() {
        if !item.tags.iter().any(|t| t == want) {
            return false;
        }
    }
    if let Some(want) = f.source {
        if item.source != want {
            return false;
        }
    }
    within_window(item.created_at, f)
}

/// Apply [`RetrievalFilters`] to a fact. Typed fields where the fact
/// has them (`type`, `source`, `created_at`); payload-key matching for
/// `host`/`repo`/`file`/`tag`, preserving the old adapter behavior
/// where a fact whose *payload* carries e.g. a `host` key matches a
/// host filter (`EnvFact` payloads routinely do).
fn fact_matches_filters(fact: &Fact, f: &RetrievalFilters) -> bool {
    if let Some(want) = f.type_ {
        if fact.fact_type != want {
            return false;
        }
    }
    if let Some(want) = f.source {
        if fact.source != want {
            return false;
        }
    }
    payload_matches_filters(&fact.payload, f) && within_window(fact.created_at, f)
}

/// Apply [`RetrievalFilters`] to an event. A `type` filter names a
/// `FactType` — events have none, so it excludes all events (as the
/// old conservative payload matching did).
fn event_matches_filters(event: &Event, f: &RetrievalFilters) -> bool {
    if f.type_.is_some() {
        return false;
    }
    if let Some(want) = f.source {
        if event.source != want {
            return false;
        }
    }
    payload_matches_filters(&event.payload, f) && within_window(event.created_at, f)
}

/// Payload-key matching for the location-shaped filters on fact/event
/// payloads (the caller-supplied JSON body, which is where such keys
/// live for those kinds).
fn payload_matches_filters(payload: &serde_json::Value, f: &RetrievalFilters) -> bool {
    let obj = payload.as_object();
    let key = |k: &str| {
        obj.and_then(|o| o.get(k))
            .and_then(serde_json::Value::as_str)
    };
    if let Some(want) = f.host.as_deref() {
        if key("host") != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.repo.as_deref() {
        if key("repo") != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.file.as_deref() {
        if key("file") != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.tag.as_deref() {
        let has = obj
            .and_then(|o| o.get("tags"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|arr| arr.iter().any(|v| v.as_str().is_some_and(|s| s == want)));
        if !has {
            return false;
        }
    }
    true
}

fn within_window(ts: time::OffsetDateTime, f: &RetrievalFilters) -> bool {
    if let Some(since) = f.since {
        if ts < since {
            return false;
        }
    }
    if let Some(until) = f.until {
        if ts > until {
            return false;
        }
    }
    true
}

/// Resolved author refs for a set of knowledge ids.
struct KnowledgeAuthors {
    author_map: HashMap<uuid::Uuid, uuid::Uuid>,
    authors: HashMap<uuid::Uuid, PublicAuthorRef>,
}

impl KnowledgeAuthors {
    fn author_ref_for(&self, id: uuid::Uuid) -> PublicAuthorRef {
        self.author_map
            .get(&id)
            .and_then(|aid| self.authors.get(aid))
            .cloned()
            .unwrap_or_else(PublicAuthorRef::unknown)
    }
}

async fn resolve_knowledge_authors<S: Store>(
    store: &Arc<S>,
    ids: &[uuid::Uuid],
) -> KnowledgeAuthors {
    let mut ids: Vec<uuid::Uuid> = ids.to_vec();
    ids.sort();
    ids.dedup();
    let author_map = store
        .knowledge_authors_by_ids(&ids)
        .await
        .unwrap_or_default();
    let mut wanted: Vec<uuid::Uuid> = author_map.values().copied().collect();
    wanted.sort();
    wanted.dedup();
    let mut authors = HashMap::with_capacity(wanted.len());
    for aid in wanted {
        if let Ok(Some(a)) = store.get_author_by_id(aid).await {
            authors.insert(aid, PublicAuthorRef::from_record(&a));
        }
    }
    KnowledgeAuthors {
        author_map,
        authors,
    }
}

/// Collapse duplicate knowledge hits in place (sprint 026, #641).
///
/// `scored` is best-first within each source, so the surviving copy is
/// the best-ranked one. Fact and event entries carry no `content_hash`
/// and pass through untouched. Each survivor is annotated with the
/// copies it absorbed, so nothing is lost — a caller that wants the copy
/// on a particular host can address it by id.
fn collapse_duplicates_in_place(scored: &mut Vec<(f32, u32, PublicMemory)>) {
    let collapsed = crate::dedupe::collapse_duplicates(
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

/// Saturating `usize -> u32` for a result-list index. A source never
/// returns more than `MAX_TOP_K` hits, so this never actually saturates.
fn rank_u32(rank: usize) -> u32 {
    u32::try_from(rank).unwrap_or(u32::MAX)
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

/// Fusion weight for a knowledge hit (sprint 029, #644): provenance
/// tier × declared-volatility age demotion. Facts and events stay at
/// 1.0 — facts already carry their own trust/decay machinery.
///
/// `boost_eligible` is the query-relative competitiveness gate
/// (`provenance::boost_threshold`): the tier weight applies only to
/// hits that are semantically competitive for THIS query; the
/// volatility demotion applies regardless (it demotes, never boosts).
fn knowledge_weight(item: &KnowledgeItem, author_agent: &str, boost_eligible: bool) -> f32 {
    let tier = ProvenanceTier::classify(item.source, Some(author_agent), item.machine.is_some());
    let tier_w = if boost_eligible { tier.weight() } else { 1.0 };
    #[allow(clippy::cast_precision_loss)]
    let age_days =
        ((time::OffsetDateTime::now_utc() - item.created_at).whole_seconds() as f32) / 86_400.0;
    tier_w * volatility_demotion(item.volatility.as_deref(), age_days)
}

/// Rank-fuse `scored` in place (sprint 024 #328): partition into
/// per-kind best-first lists, RRF-fuse via [`crate::hybrid::fuse`],
/// then reorder the entries by the fused ranking and set each `score` to
/// its fused value. RRF keys on id + within-source rank, so equal
/// within-source ranks land at comparable positions regardless of kind
/// — knowledge no longer structurally outranks facts/events.
///
/// Sprint 029 (#644): two additions. `weights` scales each hit's RRF
/// contribution (provenance × volatility; absent ids stay neutral), and
/// `curated_order` — the curated stratum's own best-first rank list —
/// enters as a 4th fusion source, so a curated memory's *stratum* rank
/// counts even when its global ANN rank is deep.
fn fuse_in_place(
    scored: &mut [(f32, u32, PublicMemory)],
    strategy: FusionStrategy,
    weights: &HashMap<uuid::Uuid, f32>,
    curated_order: &[uuid::Uuid],
    lexical_order: &[uuid::Uuid],
) {
    let weight_of = |id: uuid::Uuid| weights.get(&id).copied().unwrap_or(1.0);
    let rank_list = |order: &[uuid::Uuid]| -> Vec<RankedRow> {
        order
            .iter()
            .map(|id| RankedRow {
                weight: weight_of(*id),
                source: RetrievalSource::Vector,
                id: *id,
                score: 0.0,
                payload: serde_json::Value::Null,
            })
            .collect()
    };
    let mut by_kind: [Vec<RankedRow>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (_, _, mem) in scored.iter() {
        let (idx, source) = match mem.kind() {
            MemoryKind::Knowledge => (0usize, RetrievalSource::Vector),
            MemoryKind::Fact => (1, RetrievalSource::Fts),
            MemoryKind::Event => (2, RetrievalSource::Fts),
        };
        by_kind[idx].push(RankedRow {
            weight: weight_of(mem.id),
            source,
            id: mem.id,
            score: 0.0,
            payload: serde_json::Value::Null,
        });
    }
    let mut sources: Vec<Vec<RankedRow>> = by_kind.into_iter().collect();
    if !curated_order.is_empty() {
        sources.push(rank_list(curated_order));
    }
    // Sprint 037 (#333): the lexical list is a 5th rank source. A hit
    // whose text carries every query token earns a contribution at its
    // lexical rank; hits present in several lists sum contributions,
    // which is RRF working as intended.
    if !lexical_order.is_empty() {
        sources.push(rank_list(lexical_order));
    }
    let fused = crate::hybrid::fuse(sources, strategy);
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

/// Second-stage cross-encoder rerank of the knowledge candidates
/// (sprint 030, #685).
///
/// Scores up to `window` knowledge entries against the query via TEI
/// `/rerank` and reorders the knowledge *source list* — the within-
/// source rank order that feeds weighted RRF — plus `curated_order`,
/// the stratum's own rank list, by the same scores. Facts and events
/// are not submitted: their payloads are JSON, not prose, and the
/// cross-encoder's scores for them would be noise.
///
/// This placement is #685's ruling: the cross-encoder fixes semantic
/// order *within* the knowledge source, then 029's provenance weights
/// and stratum boost apply to that reranked order at fusion — it
/// complements the provenance work rather than replacing it. In
/// particular, two same-tier curated hits (the 029 known-open shape:
/// target stuck at rank 1 behind a sibling memory) are exactly what
/// per-hit weights cannot separate and a cross-encoder can.
///
/// Best-effort by contract: on any reranker error the un-reranked
/// order is served and a warning logged. A search must never fail
/// because an optional quality stage is sick.
async fn rerank_stage(
    reranker: &TeiReranker,
    query: &str,
    scored: &mut Vec<(f32, u32, PublicMemory)>,
    curated_order: &mut [uuid::Uuid],
    lexical_order: &mut [uuid::Uuid],
    window: usize,
    transport: &'static str,
) {
    let candidates: Vec<(uuid::Uuid, &str)> = scored
        .iter()
        .filter(|(_, _, m)| m.kind() == MemoryKind::Knowledge)
        .map(|(_, _, m)| {
            let text = match &m.content {
                klams_types::PublicMemoryContent::Knowledge { text, .. } => text.as_str(),
                _ => "",
            };
            (m.id, text)
        })
        .take(window)
        .collect();
    if candidates.len() < 2 {
        return; // nothing to reorder
    }
    let _guard = crate::metrics::LatencyGuard::retrieval("rerank", transport);
    let texts: Vec<&str> = candidates.iter().map(|(_, t)| *t).collect();
    match reranker.rerank(query, &texts).await {
        Ok(hits) => {
            let window_ids: Vec<uuid::Uuid> = candidates.iter().map(|(id, _)| *id).collect();
            apply_rerank_order(scored, curated_order, lexical_order, &window_ids, &hits);
        }
        Err(e) => {
            crate::metrics::incr_rerank_skipped();
            tracing::warn!(%e, "rerank stage skipped (serving un-reranked order)");
        }
    }
}

/// Apply a rerank result: the knowledge subsequence of `scored` is
/// permuted so the reranked window leads in cross-encoder order,
/// followed by any beyond-window knowledge entries in their prior
/// order; knowledge `source_rank`s are renumbered to the new order and
/// `curated_order` / `lexical_order` are sorted by it. Facts and events
/// keep both their entries and their ranks untouched.
fn apply_rerank_order(
    scored: &mut Vec<(f32, u32, PublicMemory)>,
    curated_order: &mut [uuid::Uuid],
    lexical_order: &mut [uuid::Uuid],
    window_ids: &[uuid::Uuid],
    hits: &[klams_store::RerankHit],
) {
    // id -> new knowledge rank: reranked window first, then the tail.
    let mut new_rank: HashMap<uuid::Uuid, usize> = hits
        .iter()
        .enumerate()
        .map(|(rank, h)| (window_ids[h.index], rank))
        .collect();
    let mut next = new_rank.len();
    for (_, _, m) in scored.iter() {
        if m.kind() == MemoryKind::Knowledge && !new_rank.contains_key(&m.id) {
            new_rank.insert(m.id, next);
            next += 1;
        }
    }
    // Partition, reorder knowledge, reassemble. The candidate stages
    // build the list as knowledge-then-facts-then-events, so this
    // preserves the original kind layout; and fusion partitions per
    // kind anyway, so only the within-kind order matters downstream.
    let mut knowledge: Vec<(f32, u32, PublicMemory)> = Vec::new();
    let mut others: Vec<(f32, u32, PublicMemory)> = Vec::new();
    for entry in scored.drain(..) {
        if entry.2.kind() == MemoryKind::Knowledge {
            knowledge.push(entry);
        } else {
            others.push(entry);
        }
    }
    knowledge.sort_by_key(|(_, _, m)| new_rank.get(&m.id).copied().unwrap_or(usize::MAX));
    for (rank, entry) in knowledge.iter_mut().enumerate() {
        entry.1 = rank_u32(rank);
    }
    scored.extend(knowledge);
    scored.extend(others);
    curated_order.sort_by_key(|id| new_rank.get(id).copied().unwrap_or(usize::MAX));
    lexical_order.sort_by_key(|id| new_rank.get(id).copied().unwrap_or(usize::MAX));
}

/// Label for the calling agent (sprint 026, #643). One helper so the
/// miss log, the sample log, and the metric counters can never disagree
/// about who ran a search — they used three different answers before
/// (`"unknown"`, nothing, and the literal `"anonymous"`).
#[must_use]
pub fn caller_label(caller: Option<&str>) -> &str {
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
                volatility: None,
                supersedes: None,
                superseded_by: None,
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
        // Per-source order (as the candidate stages build it): knowledge
        // best-first, then facts best-first. Raw cosine >> ts_rank, so
        // the OLD raw sort put both knowledge hits above both facts. RRF
        // must interleave by rank: fact@rank0 outranks knowledge@rank1.
        let k0 = mem(MemoryKind::Knowledge, "k0");
        let k1 = mem(MemoryKind::Knowledge, "k1");
        let f0 = mem(MemoryKind::Fact, "f0");
        let (k0id, k1id, f0id) = (k0.id, k1.id, f0.id);
        let mut scored = vec![
            (0.91_f32, 0, k0), // knowledge rank 0, high cosine
            (0.86_f32, 1, k1), // knowledge rank 1
            (0.05_f32, 0, f0), // fact rank 0, tiny ts_rank
        ];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &HashMap::new(),
            &[],
            &[],
        );
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

    // ---- Sprint 029 (#644): provenance weights + the curated stratum
    // as a 4th fusion source.

    #[test]
    fn a_curated_hit_at_rank_two_outranks_bulk_with_provenance_weight() {
        // The #628 known-open shape, verbatim: the hand-authored gotcha
        // is retrieved but sits at rank 2 behind two bulk chunks. Its
        // hand-authored weight (2.0) must invert that.
        let b0 = mem(MemoryKind::Knowledge, "b0");
        let b1 = mem(MemoryKind::Knowledge, "b1");
        let gotcha = mem(MemoryKind::Knowledge, "gotcha");
        let gid = gotcha.id;
        let mut weights = HashMap::new();
        weights.insert(gid, 2.0_f32);
        let mut scored = vec![(0.71_f32, 0, b0), (0.69_f32, 1, b1), (0.66_f32, 2, gotcha)];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &weights,
            &[],
            &[],
        );
        assert_eq!(
            scored[0].2.id, gid,
            "hand-authored weight must lift the gotcha over bulk (2/63 > 1/61)"
        );
    }

    #[test]
    fn the_curated_stratum_lifts_a_hit_the_global_page_ranked_deep() {
        // A curated memory at the BOTTOM of the global list (rank 3)
        // with only a mild weight: the stratum's own rank-0 entry gives
        // it a second RRF contribution, which must carry it to the top.
        let b0 = mem(MemoryKind::Knowledge, "b0");
        let b1 = mem(MemoryKind::Knowledge, "b1");
        let b2 = mem(MemoryKind::Knowledge, "b2");
        let cur = mem(MemoryKind::Knowledge, "cur");
        let cid = cur.id;
        let mut weights = HashMap::new();
        weights.insert(cid, 1.5_f32);
        let mut scored = vec![
            (0.70_f32, 0, b0),
            (0.68_f32, 1, b1),
            (0.67_f32, 2, b2),
            (0.60_f32, 3, cur),
        ];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &weights,
            &[cid],
            &[],
        );
        assert_eq!(
            scored[0].2.id, cid,
            "stratum rank-0 + weight 1.5 must beat bulk rank-0 (1.5/64 + 1.5/61 > 1/61)"
        );
    }

    // ---- Sprint 037 (#333): the lexical list as a 5th fusion source.

    #[test]
    fn a_lexical_only_hit_lands_via_the_lexical_list() {
        // The measured "klams gotcha" shape (2026-07-30): the curated
        // gotcha's cosine (0.41) is uncompetitive with bulk (0.49), so
        // it enters the knowledge list only by lexical appendage at the
        // tail — but its lexical rank-0 contribution must carry it to
        // the top of the page (1/65 + 1/61 > 1/61).
        let b0 = mem(MemoryKind::Knowledge, "b0");
        let b1 = mem(MemoryKind::Knowledge, "b1");
        let b2 = mem(MemoryKind::Knowledge, "b2");
        let b3 = mem(MemoryKind::Knowledge, "b3");
        let lex = mem(MemoryKind::Knowledge, "lex");
        let lid = lex.id;
        let mut scored = vec![
            (0.49_f32, 0, b0),
            (0.47_f32, 1, b1),
            (0.46_f32, 2, b2),
            (0.45_f32, 3, b3),
            (0.41_f32, 4, lex), // appended lexical-only tail entry
        ];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &HashMap::new(),
            &[],
            &[lid],
        );
        assert_eq!(
            scored[0].2.id, lid,
            "lexical rank-0 + knowledge tail rank must beat bulk rank-0"
        );
    }

    #[test]
    fn a_hit_on_both_lists_sums_contributions_without_duplicating() {
        // A hit present in the global page AND the lexical list keeps
        // one entry whose fused score carries both contributions.
        let a = mem(MemoryKind::Knowledge, "a");
        let b = mem(MemoryKind::Knowledge, "b");
        let (aid, bid) = (a.id, b.id);
        let mut scored = vec![(0.60_f32, 0, a), (0.55_f32, 1, b)];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &HashMap::new(),
            &[],
            &[bid],
        );
        assert_eq!(scored.len(), 2, "no duplicate entry for the shared id");
        assert_eq!(
            scored[0].2.id, bid,
            "knowledge rank-1 + lexical rank-0 (1/62 + 1/61) must beat bare rank-0 (1/61)"
        );
        assert_eq!(scored[1].2.id, aid);
    }

    #[test]
    fn neutral_weights_and_no_stratum_change_nothing() {
        // Guard: with no curated hits in play the 029 seam must be a
        // no-op relative to 024's fusion.
        let k0 = mem(MemoryKind::Knowledge, "k0");
        let k1 = mem(MemoryKind::Knowledge, "k1");
        let (i0, i1) = (k0.id, k1.id);
        let mut scored = vec![(0.9_f32, 0, k0), (0.8_f32, 1, k1)];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &HashMap::new(),
            &[],
            &[],
        );
        assert_eq!(scored[0].2.id, i0);
        assert_eq!(scored[1].2.id, i1);
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
            volatility: None,
            supersedes: None,
            superseded_by: None,
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
                volatility: None,
                supersedes: None,
                superseded_by: None,
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

    // ---- Sprint 030 (#685): second-stage rerank order application.

    use klams_store::RerankHit;

    #[test]
    fn rerank_reorders_knowledge_and_renumbers_ranks() {
        let k0 = mem(MemoryKind::Knowledge, "k0");
        let k1 = mem(MemoryKind::Knowledge, "k1");
        let k2 = mem(MemoryKind::Knowledge, "k2");
        let ids = [k0.id, k1.id, k2.id];
        let mut scored = vec![(0.7_f32, 0, k0), (0.6_f32, 1, k1), (0.5_f32, 2, k2)];
        // Cross-encoder disagrees with the cosine order: k2 > k0 > k1.
        let hits = [
            RerankHit {
                index: 2,
                score: 0.9,
            },
            RerankHit {
                index: 0,
                score: 0.5,
            },
            RerankHit {
                index: 1,
                score: 0.1,
            },
        ];
        let mut curated: Vec<uuid::Uuid> = Vec::new();
        apply_rerank_order(&mut scored, &mut curated, &mut [], &ids, &hits);
        assert_eq!(
            scored.iter().map(|(_, _, m)| m.id).collect::<Vec<_>>(),
            vec![ids[2], ids[0], ids[1]]
        );
        assert_eq!(
            scored.iter().map(|(_, r, _)| *r).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "knowledge source_ranks must be renumbered to the reranked order"
        );
        // raw scores ride along untouched — raw_score stays the cosine.
        assert!((scored[0].0 - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn rerank_reorders_the_curated_stratum_list_too() {
        // The 029 known-open shape this sprint exists for: two curated
        // siblings, the true target stuck at stratum rank 1. The
        // cross-encoder's order must flow into curated_order so the 4th
        // fusion source stops double-boosting the wrong sibling.
        let sib = mem(MemoryKind::Knowledge, "sibling");
        let target = mem(MemoryKind::Knowledge, "target");
        let ids = [sib.id, target.id];
        let mut scored = vec![(0.66_f32, 0, sib), (0.64_f32, 1, target)];
        let mut curated = vec![ids[0], ids[1]];
        let hits = [
            RerankHit {
                index: 1,
                score: 0.95,
            },
            RerankHit {
                index: 0,
                score: 0.40,
            },
        ];
        apply_rerank_order(&mut scored, &mut curated, &mut [], &ids, &hits);
        assert_eq!(curated, vec![ids[1], ids[0]]);
        assert_eq!(scored[0].2.id, ids[1]);
    }

    #[test]
    fn rerank_leaves_facts_and_events_untouched() {
        let k0 = mem(MemoryKind::Knowledge, "k0");
        let k1 = mem(MemoryKind::Knowledge, "k1");
        let f0 = mem(MemoryKind::Fact, "f0");
        let e0 = mem(MemoryKind::Event, "e0");
        let kids = [k0.id, k1.id];
        let (fid, eid) = (f0.id, e0.id);
        let mut scored = vec![
            (0.7_f32, 0, k0),
            (0.6_f32, 1, k1),
            (0.05_f32, 0, f0),
            (0.04_f32, 0, e0),
        ];
        let hits = [
            RerankHit {
                index: 1,
                score: 0.9,
            },
            RerankHit {
                index: 0,
                score: 0.1,
            },
        ];
        apply_rerank_order(&mut scored, &mut Vec::new(), &mut [], &kids, &hits);
        // Kind layout preserved (knowledge, then facts, then events),
        // and the non-knowledge entries keep their ranks.
        assert_eq!(
            scored.iter().map(|(_, _, m)| m.id).collect::<Vec<_>>(),
            vec![kids[1], kids[0], fid, eid]
        );
        assert_eq!(scored[2].1, 0);
        assert_eq!(scored[3].1, 0);
    }

    #[test]
    fn beyond_window_knowledge_keeps_its_order_after_the_window() {
        // Only k0/k1 were in the rerank window; k2 was beyond it and
        // must trail the reranked window in its prior relative order.
        let k0 = mem(MemoryKind::Knowledge, "k0");
        let k1 = mem(MemoryKind::Knowledge, "k1");
        let k2 = mem(MemoryKind::Knowledge, "k2");
        let ids = [k0.id, k1.id, k2.id];
        let mut scored = vec![(0.7_f32, 0, k0), (0.6_f32, 1, k1), (0.5_f32, 2, k2)];
        let window = [ids[0], ids[1]];
        let hits = [
            RerankHit {
                index: 1,
                score: 0.9,
            },
            RerankHit {
                index: 0,
                score: 0.1,
            },
        ];
        apply_rerank_order(&mut scored, &mut Vec::new(), &mut [], &window, &hits);
        assert_eq!(
            scored.iter().map(|(_, _, m)| m.id).collect::<Vec<_>>(),
            vec![ids[1], ids[0], ids[2]]
        );
    }

    #[test]
    fn rrf_output_is_sorted_by_fused_score_descending() {
        let mut scored = vec![
            (0.9_f32, 0, mem(MemoryKind::Knowledge, "k0")),
            (0.8_f32, 1, mem(MemoryKind::Knowledge, "k1")),
            (0.07_f32, 0, mem(MemoryKind::Fact, "f0")),
            (0.03_f32, 1, mem(MemoryKind::Event, "e0")),
        ];
        fuse_in_place(
            &mut scored,
            FusionStrategy::default_rrf(),
            &HashMap::new(),
            &[],
            &[],
        );
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

    // ---- Sprint 036 (#730): typed filter application on the shared
    // pipeline. The old REST adapter matched payload JSON keys; these
    // pin the typed semantics, including the two documented
    // conservative-drop rules (a `type` filter excludes knowledge and
    // events wholesale — only facts have a FactType).

    use klams_types::{FactType, Source};

    fn kitem(host: Option<&str>, repo: Option<&str>, tags: &[&str]) -> KnowledgeItem {
        KnowledgeItem {
            id: uuid::Uuid::now_v7(),
            text: "t".into(),
            content_hash: "h".into(),
            source: Source::Task,
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
            repo: repo.map(str::to_string),
            file: Some("/tmp/f.md".into()),
            machine: host.map(str::to_string),
            machines: Vec::new(),
            heading_path: None,
            language: None,
            chunk_index: None,
            volatility: None,
            supersedes: None,
            superseded_by: None,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn knowledge_filters_match_on_typed_fields() {
        let item = kitem(Some("kubs0"), Some("klams"), &["gpu"]);
        let f = RetrievalFilters {
            host: Some("kubs0".into()),
            repo: Some("klams".into()),
            tag: Some("gpu".into()),
            ..Default::default()
        };
        assert!(knowledge_matches_filters(&item, &f));
        let wrong_host = RetrievalFilters {
            host: Some("kai".into()),
            ..Default::default()
        };
        assert!(!knowledge_matches_filters(&item, &wrong_host));
    }

    #[test]
    fn a_type_filter_excludes_knowledge_and_events_but_not_matching_facts() {
        // Only facts carry a FactType; the filter is conservative for
        // the other kinds, exactly like the old payload-key matching.
        let item = kitem(None, None, &[]);
        let f = RetrievalFilters {
            type_: Some(FactType::EnvFact),
            ..Default::default()
        };
        assert!(!knowledge_matches_filters(&item, &f));

        let fact = Fact {
            id: uuid::Uuid::now_v7(),
            fact_type: FactType::EnvFact,
            payload: serde_json::json!({"k": "v"}),
            version: 1,
            source: Source::Task,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            dissent_count: 0,
            last_used_at: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
        };
        assert!(fact_matches_filters(&fact, &f));
        let other = RetrievalFilters {
            type_: Some(FactType::UserFact),
            ..Default::default()
        };
        assert!(!fact_matches_filters(&fact, &other));

        let event = Event {
            id: uuid::Uuid::now_v7(),
            task_id: None,
            category: "Service".into(),
            payload: serde_json::json!({}),
            source: Source::Task,
            created_at: time::OffsetDateTime::now_utc(),
        };
        assert!(!event_matches_filters(&event, &f));
    }

    #[test]
    fn a_fact_payload_host_key_still_matches_a_host_filter() {
        // EnvFacts routinely carry a `host` key in their payload; the
        // old adapter matched it and the typed pipeline must keep that.
        let fact = Fact {
            id: uuid::Uuid::now_v7(),
            fact_type: FactType::EnvFact,
            payload: serde_json::json!({"host": "kubs0", "gpu": "4080"}),
            version: 1,
            source: Source::Task,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            dissent_count: 0,
            last_used_at: None,
            created_at: time::OffsetDateTime::now_utc(),
            updated_at: time::OffsetDateTime::now_utc(),
        };
        let f = RetrievalFilters {
            host: Some("kubs0".into()),
            ..Default::default()
        };
        assert!(fact_matches_filters(&fact, &f));
        let wrong = RetrievalFilters {
            host: Some("kai".into()),
            ..Default::default()
        };
        assert!(!fact_matches_filters(&fact, &wrong));
    }

    #[test]
    fn since_until_filter_knowledge_by_created_at() {
        // The old adapter dropped EVERY knowledge row under a time
        // filter (its payload carried no timestamp key). Typed matching
        // uses created_at — a strict improvement, documented in the
        // sprint record.
        let mut item = kitem(None, None, &[]);
        item.created_at = time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(1000);
        let inside = RetrievalFilters {
            since: Some(time::OffsetDateTime::UNIX_EPOCH),
            until: Some(time::OffsetDateTime::UNIX_EPOCH + time::Duration::days(2000)),
            ..Default::default()
        };
        assert!(knowledge_matches_filters(&item, &inside));
        let before = RetrievalFilters {
            until: Some(time::OffsetDateTime::UNIX_EPOCH),
            ..Default::default()
        };
        assert!(!knowledge_matches_filters(&item, &before));
    }

    #[test]
    fn filters_active_spots_any_set_field() {
        assert!(!filters_active(&RetrievalFilters::default()));
        assert!(filters_active(&RetrievalFilters {
            tag: Some("x".into()),
            ..Default::default()
        }));
        assert!(filters_active(&RetrievalFilters {
            since: Some(time::OffsetDateTime::UNIX_EPOCH),
            ..Default::default()
        }));
    }

    // ---- Sprint 036 (#730): validation lives in the core now, so both
    // surfaces inherit the same limits (REST had no MAX_QUERY_LEN).

    #[tokio::test]
    async fn validation_rejects_empty_and_oversized_queries_and_bad_top_k() {
        let store = std::sync::Arc::new(NoopStore);
        let cfg = RetrievalConfig::default();
        let err = search(
            &store,
            SearchParams::query("   "),
            &cfg,
            SourceTolerance::HardFail,
            None,
            "test",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RetrievalError::EmptyQuery));

        let err = search(
            &store,
            SearchParams::query("q".repeat(MAX_QUERY_LEN + 1)),
            &cfg,
            SourceTolerance::HardFail,
            None,
            "test",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RetrievalError::QueryTooLong { .. }));

        let mut params = SearchParams::query("ok");
        params.top_k = Some(0);
        let err = search(
            &store,
            params,
            &cfg,
            SourceTolerance::HardFail,
            None,
            "test",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RetrievalError::InvalidTopK { .. }));
    }

    #[tokio::test]
    async fn degrade_tolerance_serves_fts_when_the_embedder_is_down() {
        // The REST contract: a sick knowledge source yields a 200 with
        // degraded=true and whatever FTS produced, never an error.
        let store = std::sync::Arc::new(NoopStore);
        let cfg = RetrievalConfig::default();
        let out = search(
            &store,
            SearchParams::query("anything"),
            &cfg,
            SourceTolerance::Degrade,
            None,
            "test",
        )
        .await
        .expect("degrade must not error");
        assert!(out.degraded, "embed failure must flag degraded");
        assert!(out.hits.is_empty());
    }

    #[tokio::test]
    async fn hard_fail_tolerance_propagates_the_embed_error() {
        // The MCP contract: the same failure is a typed error carrying
        // the store taxonomy.
        let store = std::sync::Arc::new(NoopStore);
        let cfg = RetrievalConfig::default();
        let err = search(
            &store,
            SearchParams::query("anything"),
            &cfg,
            SourceTolerance::HardFail,
            None,
            "test",
        )
        .await
        .unwrap_err();
        assert!(matches!(err, RetrievalError::Embed(_)));
    }

    #[tokio::test]
    async fn the_lexical_list_surfaces_a_token_match_the_global_page_missed() {
        // End-to-end through `search`: the store's ANN page holds three
        // bulk hits; the target exists only in the lexical list (its
        // cosine is real but uncompetitive — the "klams gotcha" failure
        // shape, measured live 2026-07-30). It must land on the page,
        // and at the top, with its raw cosine preserved.
        let store = std::sync::Arc::new(LexicalGapStore);
        let cfg = RetrievalConfig::default();
        let out = search(
            &store,
            SearchParams::query("klams gotcha"),
            &cfg,
            SourceTolerance::HardFail,
            None,
            "test",
        )
        .await
        .expect("search must succeed");
        assert_eq!(out.hits.len(), 4, "three bulk + one lexical-only hit");
        let top = &out.hits[0];
        assert_eq!(
            top.memory.id,
            lexical_target_id(),
            "the lexical-only hit must fuse to the top of the page"
        );
        assert_eq!(
            top.raw_score,
            Some(0.41),
            "raw_score stays the pre-fusion cosine"
        );
        let ids: Vec<uuid::Uuid> = out.hits.iter().map(|h| h.memory.id).collect();
        let mut deduped = ids.clone();
        deduped.dedup();
        assert_eq!(ids, deduped, "no duplicate entries");
    }

    fn lexical_target_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(0x0197_0000_0000_0000_0000_0000_0000_0042)
    }

    fn lex_item(id_low: u128, text: &str) -> KnowledgeItem {
        let mut item = kitem(None, None, &[]);
        item.id = uuid::Uuid::from_u128(0x0197_0000_0000_0000_0000_0000_0000_0000 | id_low);
        item.text = text.to_string();
        item.content_hash = format!("hash-{id_low}");
        item
    }

    /// Store modeling the lexical-gap corpus: a competitive bulk ANN
    /// page that misses the target, an empty curated stratum (the
    /// target's cosine is below the boost threshold, as measured), and
    /// a lexical list that finds it by literal token match.
    struct LexicalGapStore;

    #[async_trait::async_trait]
    impl Store for LexicalGapStore {
        async fn upsert_fact_v2(
            &self,
            _req: klams_types::UpsertFact,
        ) -> klams_store::StoreResult<klams_types::FactWriteOutcome> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn append_event(
            &self,
            _req: klams_types::AppendEvent,
        ) -> klams_store::StoreResult<Event> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn index_knowledge(
            &self,
            _req: klams_types::IndexKnowledge,
        ) -> klams_store::StoreResult<KnowledgeItem> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn list_facts(
            &self,
            _q: klams_store::FactQuery,
        ) -> klams_store::StoreResult<(Vec<Fact>, Option<String>)> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn list_events(
            &self,
            _q: klams_store::EventQuery,
        ) -> klams_store::StoreResult<(Vec<Event>, Option<String>)> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn search_knowledge(
            &self,
            _v: Vec<f32>,
            _k: u32,
        ) -> klams_store::StoreResult<Vec<(KnowledgeItem, f32)>> {
            Ok(vec![
                (lex_item(1, "bulk chunk about klams generally"), 0.49),
                (lex_item(2, "another bulk klams chunk"), 0.47),
                (lex_item(3, "a third bulk klams chunk"), 0.46),
            ])
        }
        async fn search_knowledge_curated(
            &self,
            _v: Vec<f32>,
            _k: u32,
        ) -> klams_store::StoreResult<Vec<(KnowledgeItem, f32)>> {
            Ok(vec![])
        }
        async fn search_knowledge_lexical(
            &self,
            query_text: &str,
            _v: Vec<f32>,
            _k: u32,
        ) -> klams_store::StoreResult<Vec<(KnowledgeItem, f32)>> {
            assert_eq!(query_text, "klams gotcha", "pipeline passes the raw query");
            let mut item = lex_item(0x42, "GOTCHA - the klams gotcha the page missed");
            item.source = Source::AgentProposal;
            Ok(vec![(item, 0.41)])
        }
        async fn search_text(
            &self,
            _q: &str,
            _k: u32,
        ) -> klams_store::StoreResult<(Vec<klams_store::TextHit>, Vec<klams_store::TextHit>)>
        {
            Ok((Vec::new(), Vec::new()))
        }
        async fn find_knowledge_by_content_hash(
            &self,
            _hash: &str,
        ) -> klams_store::StoreResult<Option<uuid::Uuid>> {
            Ok(None)
        }
        async fn get_knowledge(
            &self,
            _id: uuid::Uuid,
        ) -> klams_store::StoreResult<Option<KnowledgeItem>> {
            Ok(None)
        }
        async fn embed_query(&self, _query: &str) -> klams_store::StoreResult<Vec<f32>> {
            Ok(vec![0.0; 4])
        }
    }

    /// Store whose embedder is down and whose FTS is empty — the
    /// minimal double for the tolerance tests. Everything else uses the
    /// trait's erroring defaults.
    struct NoopStore;

    #[async_trait::async_trait]
    impl Store for NoopStore {
        async fn upsert_fact_v2(
            &self,
            _req: klams_types::UpsertFact,
        ) -> klams_store::StoreResult<klams_types::FactWriteOutcome> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn append_event(
            &self,
            _req: klams_types::AppendEvent,
        ) -> klams_store::StoreResult<Event> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn index_knowledge(
            &self,
            _req: klams_types::IndexKnowledge,
        ) -> klams_store::StoreResult<KnowledgeItem> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn list_facts(
            &self,
            _q: klams_store::FactQuery,
        ) -> klams_store::StoreResult<(Vec<Fact>, Option<String>)> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn list_events(
            &self,
            _q: klams_store::EventQuery,
        ) -> klams_store::StoreResult<(Vec<Event>, Option<String>)> {
            Err(StoreError::Other("not implemented".into()))
        }
        async fn search_knowledge(
            &self,
            _v: Vec<f32>,
            _k: u32,
        ) -> klams_store::StoreResult<Vec<(KnowledgeItem, f32)>> {
            Ok(Vec::new())
        }
        async fn search_text(
            &self,
            _q: &str,
            _k: u32,
        ) -> klams_store::StoreResult<(Vec<klams_store::TextHit>, Vec<klams_store::TextHit>)>
        {
            Ok((Vec::new(), Vec::new()))
        }
        async fn find_knowledge_by_content_hash(
            &self,
            _hash: &str,
        ) -> klams_store::StoreResult<Option<uuid::Uuid>> {
            Ok(None)
        }
        async fn get_knowledge(
            &self,
            _id: uuid::Uuid,
        ) -> klams_store::StoreResult<Option<KnowledgeItem>> {
            Ok(None)
        }
        async fn embed_query(&self, _query: &str) -> klams_store::StoreResult<Vec<f32>> {
            Err(StoreError::Embedding("embedder down".into()))
        }
    }
}
