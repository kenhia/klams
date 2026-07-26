//! Storage adapters: Postgres (facts/events), Qdrant (knowledge),
//! and TEI embeddings.
//!
//! Exposes a `Store` trait that the worker pool drives, plus a
//! `CompositeStore` that wires the three backends together.

use async_trait::async_trait;
use klams_types::{
    AppendEvent, Dissent, DissentStatus, Event, Fact, FactType, FactWriteOutcome, IndexKnowledge,
    KnowledgeItem, Source, UpsertFact,
};
use std::error::Error;
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

pub mod backfill_qdrant_authors;
pub mod backup;
pub mod composite;
pub mod embeddings;
pub mod postgres;
pub mod qdrant;
pub mod repair;
pub mod rerank;

pub use composite::CompositeStore;
pub use embeddings::{Embedder, OpenAiCompatEmbedder, TeiEmbedder};
pub use postgres::PostgresStore;
pub use qdrant::QdrantStore;
pub use rerank::{RerankHit, TeiReranker};

/// Whether retrying the same operation could plausibly succeed.
///
/// Sprint 027 (WI #629, review F-3.1). Before this existed, klams got the
/// question wrong in *both* directions: a permanent HTTP 413 from the
/// embedder was retried three times and reported as
/// `EMBEDDING_UNAVAILABLE` + `retry_after_seconds`, while a transient
/// Postgres pool exhaustion surfaced as a bare `INTERNAL_ERROR` with no
/// retry hint at all. The classification has to live on the error itself,
/// because by the time a caller sees it the evidence is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transience {
    /// The condition is expected to clear on its own: connection
    /// refused, a 5xx, a timeout, an exhausted pool. Retrying is
    /// reasonable and `retry_after_seconds` is honest.
    Transient,
    /// The condition is a property of the request. Retrying it
    /// unchanged will fail identically, forever. Never attach a retry
    /// hint to one of these.
    Permanent,
}

#[derive(Debug)]
pub enum StoreError {
    /// A backend failed in a way that will not fix itself (bad SQL,
    /// constraint violation, malformed response).
    Backend(String),
    /// A backend is momentarily unreachable or over capacity — pool
    /// exhaustion, connection reset, a 5xx from Qdrant. Distinct from
    /// [`StoreError::Backend`] purely so callers can offer an honest
    /// retry hint (sprint 027, the mirror half of #629).
    BackendUnavailable(String),
    Conflict(String),
    /// The embedder is unavailable or failed transiently.
    Embedding(String),
    /// The embedder refused this input and will refuse it again —
    /// an unsupported content type, a malformed request, any permanent
    /// 4xx that is not a size problem.
    EmbeddingRejected(String),
    /// The input exceeds the embedder's token ceiling, with the numbers
    /// needed to split it correctly on the first retry rather than by
    /// bisection (sprint 027, #629/#632).
    PayloadTooLarge {
        oversize: klams_types::Oversize,
        /// Where the rejection came from — the local gate, or the
        /// embedder's own response body.
        detail: String,
    },
    Other(String),
    /// Sprint-002 US2: optimistic-version mismatch on a canonical
    /// fact write or dissent promote. Maps to HTTP 409 with the
    /// current stored version in the response body.
    VersionConflict {
        current_version: i32,
    },
    /// Sprint-002 US2: the targeted dissent is no longer `pending`
    /// (promoted / discarded / orphaned). Maps to HTTP 410 Gone.
    Gone(String),
}

impl StoreError {
    /// Classify a `sqlx` failure so transient database trouble is
    /// reported as retryable and everything else is not.
    ///
    /// Used at every Postgres call site in place of the old blanket
    /// `Backend(format!("ctx: {e}"))`, which erased the distinction.
    pub fn from_sqlx(ctx: &str, e: &sqlx::Error) -> Self {
        let transient = match e {
            // The pool could not hand out a connection in time, or the
            // socket died mid-flight. Both clear on their own.
            sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::Tls(_) => true,
            sqlx::Error::Database(db) => db.code().is_some_and(|c| is_transient_sqlstate(&c)),
            _ => false,
        };
        let msg = format!("{ctx}: {e}");
        if transient {
            StoreError::BackendUnavailable(msg)
        } else {
            StoreError::Backend(msg)
        }
    }

    /// Whether retrying this operation unchanged could succeed.
    pub fn transience(&self) -> Transience {
        match self {
            StoreError::BackendUnavailable(_) | StoreError::Embedding(_) => Transience::Transient,
            StoreError::Backend(_)
            | StoreError::Conflict(_)
            | StoreError::EmbeddingRejected(_)
            | StoreError::PayloadTooLarge { .. }
            | StoreError::Other(_)
            | StoreError::VersionConflict { .. }
            | StoreError::Gone(_) => Transience::Permanent,
        }
    }

    /// Convenience predicate for the many call sites that only need to
    /// decide whether to attach `retry_after_seconds`.
    pub fn is_transient(&self) -> bool {
        self.transience() == Transience::Transient
    }
}

/// SQLSTATE classes that describe a condition worth retrying.
///
/// `08` connection exception, `40` transaction rollback (deadlock and
/// serialization failure — retrying is the documented remedy), `53`
/// insufficient resources (out of connections/memory/disk), `57`
/// operator intervention (admin shutdown, cancelled statement).
fn is_transient_sqlstate(code: &str) -> bool {
    matches!(code.get(..2), Some("08" | "40" | "53" | "57"))
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Backend(m) => write!(f, "store backend error: {m}"),
            StoreError::BackendUnavailable(m) => write!(f, "store backend unavailable: {m}"),
            StoreError::Conflict(m) => write!(f, "store conflict: {m}"),
            StoreError::Embedding(m) => write!(f, "embedding error: {m}"),
            StoreError::EmbeddingRejected(m) => write!(f, "embedding rejected input: {m}"),
            StoreError::PayloadTooLarge { oversize, detail } => {
                write!(f, "payload too large: {oversize} [{detail}]")
            }
            StoreError::Other(m) => write!(f, "store error: {m}"),
            StoreError::VersionConflict { current_version } => {
                write!(f, "version conflict: current_version={current_version}")
            }
            StoreError::Gone(m) => write!(f, "gone: {m}"),
        }
    }
}

impl Error for StoreError {}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, Clone, Default)]
pub struct FactQuery {
    pub fact_type: Option<FactType>,
    pub source: Option<Source>,
    pub created_after: Option<OffsetDateTime>,
    pub created_before: Option<OffsetDateTime>,
    pub limit: u32,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EventQuery {
    pub task_id: Option<Uuid>,
    /// Raw `task_id` string for `payload->>'task_id'` matching (sprint 003 index).
    pub payload_task_id: Option<String>,
    pub category: Option<String>,
    /// Filter on `payload->>'service'`.
    pub service: Option<String>,
    pub created_after: Option<OffsetDateTime>,
    pub created_before: Option<OffsetDateTime>,
    pub limit: u32,
    pub cursor: Option<String>,
}

/// Filter set for `list_dissents`.
#[derive(Debug, Clone, Default)]
pub struct DissentQuery {
    pub fact_id: Option<Uuid>,
    pub status: Option<DissentStatus>,
    pub source: Option<Source>,
    pub created_after: Option<OffsetDateTime>,
    pub created_before: Option<OffsetDateTime>,
    pub limit: u32,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TextHit {
    pub id: Uuid,
    pub score: f32,
    pub payload: serde_json::Value,
}

/// A `memory_search` that returned nothing useful — the tuning-data
/// record inserted by the MCP search path (sprint 021, #317). `reason`
/// is `"zero_hit"` (no results) or `"low_score"` (only a weak top
/// match); `top_score` is `None` for a zero-hit.
#[derive(Debug, Clone)]
pub struct SearchMiss {
    pub query: String,
    pub caller: String,
    pub reason: String,
    pub top_score: Option<f32>,
    pub hit_count: i32,
    pub kinds: String,
}

/// A knowledge write refused for exceeding the embedder's ceiling
/// (sprint 027, #656).
///
/// Carries the full rejected `text` on purpose: it is the "what did we
/// lose" corpus. Everything else here is what makes the loss
/// *interpretable* — who hit it, by how much, and against which ceiling,
/// since the ceiling itself moves when the model changes.
#[derive(Debug, Clone)]
pub struct OversizeWrite {
    pub author_id: Option<Uuid>,
    pub agent_name: String,
    pub submitted_chars: i32,
    pub estimated_tokens: i32,
    pub limit_tokens: i32,
    pub max_chars: i32,
    pub text: String,
}

impl OversizeWrite {
    /// Build a log row from the gate's verdict.
    #[must_use]
    pub fn from_oversize(
        oversize: &klams_types::Oversize,
        author_id: Option<Uuid>,
        agent_name: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        Self {
            author_id,
            agent_name: agent_name.into(),
            // Sizes are bounded by the request the caller already sent,
            // so a saturating cast cannot lose anything meaningful.
            submitted_chars: i32::try_from(oversize.submitted_chars).unwrap_or(i32::MAX),
            estimated_tokens: i32::try_from(oversize.estimated_tokens).unwrap_or(i32::MAX),
            limit_tokens: i32::try_from(oversize.limit_tokens).unwrap_or(i32::MAX),
            max_chars: i32::try_from(oversize.max_chars).unwrap_or(i32::MAX),
            text: text.into(),
        }
    }
}

/// A record of one search, hit or miss (sprint 026, #643).
///
/// The miss log ([`SearchMiss`]) only records *failures*, and until this
/// sprint its threshold was mis-calibrated badly enough to record almost
/// nothing — so klams had no record of what agents ask it. This is that
/// record: it is what future eval queries get mined from, and what the
/// next miss-log threshold gets calibrated against (the current one came
/// from a handful of data points in #628).
///
/// `top_raw_score` is deliberately the **pre-fusion** per-source score
/// (Qdrant cosine / Postgres `ts_rank`), not the fused RRF value — RRF
/// discards magnitude, so a distribution over fused scores would say
/// nothing about match quality. `top_kind` is stored alongside because
/// the raw scales are not comparable across kinds.
#[derive(Debug, Clone)]
pub struct SearchSample {
    pub query: String,
    pub caller: String,
    pub top_raw_score: Option<f32>,
    pub top_kind: Option<String>,
    pub hit_count: i32,
    pub kinds: String,
    /// How many duplicate hits query-time collapse (#641) removed from
    /// this search — the dedupe's live effect, per query.
    pub duplicates_collapsed: i32,
}

/// Single trait the worker pool uses for all persistence.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn upsert_fact(&self, req: UpsertFact) -> StoreResult<Fact>;
    /// Sprint-002 canonical write entry point. Returns a tagged
    /// `FactWriteOutcome` so the handler can map persisted / dissented
    /// / version-conflict to the right HTTP status. Default impl
    /// delegates to `upsert_fact` and always returns `Persisted` for
    /// mock stores; the real Postgres impl overrides this.
    async fn upsert_fact_v2(&self, req: UpsertFact) -> StoreResult<FactWriteOutcome> {
        self.upsert_fact(req)
            .await
            .map(|f| FactWriteOutcome::Persisted { fact: f })
    }
    async fn append_event(&self, req: AppendEvent) -> StoreResult<Event>;
    async fn index_knowledge(&self, req: IndexKnowledge) -> StoreResult<KnowledgeItem>;
    async fn list_facts(&self, q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)>;
    async fn list_events(&self, q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)>;
    async fn search_knowledge(
        &self,
        query_vector: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>>;
    async fn search_text(
        &self,
        query: &str,
        top_k: u32,
    ) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)>;
    /// Find an existing **live** knowledge point with `hash`.
    ///
    /// Content-only since sprint 028 (#642): identical content is ONE
    /// point wherever it appears; per-file/per-host identity lives in
    /// the point's copy bookkeeping (see [`Self::attach_knowledge_copy`]).
    /// Soft-deleted points never match — a scanner chunk deduping onto a
    /// deleted memory would make live content unsearchable.
    async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>>;
    /// Record that (`machine`, `file`) also holds the content of point
    /// `id` (sprint 028 #642). Returns `true` when the copy was newly
    /// attached, `false` when it was already recorded (or the point is
    /// gone). Default is a no-op `Ok(false)` — copy bookkeeping is a
    /// Qdrant concern and mocks don't track it.
    async fn attach_knowledge_copy(
        &self,
        _id: Uuid,
        _machine: Option<&str>,
        _file: Option<&str>,
        _repo: Option<&str>,
    ) -> StoreResult<bool> {
        Ok(false)
    }
    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>>;
    /// Sprint 003 T010b: delete every knowledge point whose payload
    /// `source_file` matches, scoped to `machine`.
    ///
    /// Sprint 028 (#642): with one point per content, this is copy
    /// bookkeeping, not point deletion — the (`machine`, `file`) copy is
    /// removed from each matching point and the point itself is deleted
    /// only when its last copy goes. Returns the number of copies
    /// removed (= points affected). Default returns `Other` so mocks
    /// need not implement.
    async fn delete_knowledge_by_source_file(
        &self,
        _source_file: &str,
        _machine: Option<&str>,
    ) -> StoreResult<u64> {
        Err(StoreError::Other(
            "delete_knowledge_by_source_file not implemented".into(),
        ))
    }
    /// Embed a free-text query into a vector compatible with
    /// [`search_knowledge`]. Implementations backed by Qdrant + TEI
    /// delegate to the embedder; mock stores can return a zero vector.
    async fn embed_query(&self, query: &str) -> StoreResult<Vec<f32>>;

    /// Check `text` against the embedder's input ceiling, using the
    /// model's real tokenizer where the backend has one (sprint 027,
    /// WI #420).
    ///
    /// The ingest handler calls this **before** returning `202`, because
    /// the write is completed asynchronously by a worker that has no
    /// reply channel: anything accepted here and refused later is lost
    /// outright, and the scanner has already advanced its cursor past it.
    ///
    /// The default implementation uses the character estimate, which is
    /// what mock stores and tokenizer-less backends get.
    ///
    /// # Errors
    /// Returns [`StoreError::PayloadTooLarge`] when the text exceeds the
    /// ceiling, carrying the numbers needed to split it correctly.
    async fn check_embed_size(
        &self,
        text: &str,
        limit: klams_types::EmbedLimit,
    ) -> StoreResult<()> {
        limit
            .check(text)
            .map_err(|oversize| StoreError::PayloadTooLarge {
                oversize,
                detail: "character estimate (no tokenizer available)".into(),
            })
    }

    // Dissent operations (sprint 002 US2). Default impls return
    // `Other` so mock stores need not implement them; the Postgres
    // adapter overrides every one.
    async fn list_dissents(&self, _q: DissentQuery) -> StoreResult<(Vec<Dissent>, Option<String>)> {
        Err(StoreError::Other("list_dissents not implemented".into()))
    }
    async fn get_dissent(&self, _id: Uuid) -> StoreResult<Option<Dissent>> {
        Err(StoreError::Other("get_dissent not implemented".into()))
    }
    /// Promote a pending dissent. Returns the updated canonical fact.
    /// `expected_version` mirrors the optimistic-concurrency token on
    /// `UpsertFact`; mismatches map to `StoreError::Conflict`.
    async fn promote_dissent(
        &self,
        _dissent_id: Uuid,
        _caller_source: Source,
        _expected_version: i32,
    ) -> StoreResult<Fact> {
        Err(StoreError::Other("promote_dissent not implemented".into()))
    }
    async fn discard_dissent(
        &self,
        _dissent_id: Uuid,
        _caller_source: Source,
    ) -> StoreResult<Dissent> {
        Err(StoreError::Other("discard_dissent not implemented".into()))
    }

    /// Cheap liveness probe for the relational store. Default returns
    /// `Ok(())` so test mocks need not implement it.
    async fn health_postgres(&self) -> StoreResult<()> {
        Ok(())
    }
    /// Cheap liveness probe for the vector store.
    async fn health_qdrant(&self) -> StoreResult<()> {
        Ok(())
    }
    /// Cheap liveness probe for the embedder service.
    async fn health_embedder(&self) -> StoreResult<()> {
        Ok(())
    }

    // Sprint 007 — viewport `/v1/authors` routes. Default impls
    // return empty data so test mocks need not connect to Postgres
    // or Qdrant.

    /// List authors with rolled-up counts. Pagination orders by
    /// `(last_seen_at DESC, id DESC)`.
    async fn list_authors_v1(
        &self,
        _q: AuthorListQuery,
    ) -> StoreResult<(Vec<AuthorWithCountsOut>, Option<String>)> {
        Ok((Vec::new(), None))
    }
    /// Fetch one author with rolled-up counts.
    async fn get_author_v1(&self, _id: Uuid) -> StoreResult<Option<AuthorWithCountsOut>> {
        Ok(None)
    }
    /// List memories authored by `author_id` (facts + events + knowledge)
    /// for `GET /v1/authors/{id}/memories`.
    async fn list_author_memories(
        &self,
        _q: AuthorMemoriesQuery,
    ) -> StoreResult<(Vec<AuthorMemoryRow>, Option<String>)> {
        Ok((Vec::new(), None))
    }

    // Sprint 008 — cross-author listing surfaces (`GET /v1/memories`
    // and `event_search`). Default impls return empty data so test
    // mocks need not connect to Postgres or Qdrant.

    /// Cross-author memories listing for `GET /v1/memories`. Returns
    /// rows ordered by `(created_at DESC, id DESC)` across all three
    /// kinds with a unified opaque cursor.
    async fn list_memories(
        &self,
        _q: ListMemoriesQuery,
    ) -> StoreResult<(Vec<ListMemoriesRow>, Option<String>)> {
        Ok((Vec::new(), None))
    }

    /// Event-only search for the `event_search` MCP tool. Returns
    /// already-projected [`klams_types::PublicMemory`] values with an
    /// opaque cursor for keyset continuation.
    async fn event_search(
        &self,
        _q: EventSearchQuery,
    ) -> StoreResult<(Vec<klams_types::PublicMemory>, Option<String>)> {
        Ok((Vec::new(), None))
    }

    /// Sprint 010 — bump `last_seen_at` for an authenticated author on
    /// an HTTP write (knowledge/facts/events), mirroring the MCP write
    /// path so background daemons (scanner/monitor) that write over HTTP
    /// show fresh activity. Default no-op (returns 0 rows affected) so
    /// test mocks need not implement it.
    async fn touch_author_last_seen_at(&self, _id: Uuid) -> StoreResult<u64> {
        Ok(0)
    }
}

/// Query for `Store::list_authors_v1`.
#[derive(Debug, Clone, Default)]
pub struct AuthorListQuery {
    pub limit: u32,
    pub agent_name: Option<String>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub cursor: Option<String>,
}

/// Public projection of an author + rolled-up activity counts
/// returned by `GET /v1/authors`.
#[derive(Debug, Clone)]
pub struct AuthorWithCountsOut {
    pub author: klams_types::AuthorRecord,
    pub writes_facts: i64,
    /// Live knowledge points authored by this author (Qdrant payload
    /// filter on `author_id` with `is_empty(deleted_at)`). Populated
    /// only by `get_author_v1` (single-author detail). For
    /// `list_authors_v1` this stays 0 to avoid N Qdrant round-trips.
    pub writes_knowledge: i64,
    pub events: i64,
    pub soft_deletes_authored: i64,
    pub restores_received: i64,
}

/// Query for `Store::list_author_memories`.
#[derive(Debug, Clone)]
pub struct AuthorMemoriesQuery {
    pub author_id: Uuid,
    pub kinds: Vec<AuthorMemoryKind>,
    pub state: AuthorMemoryStateQuery,
    pub limit: u32,
    pub cursor: Option<String>,
}

/// Which memory kinds to include in the author drilldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorMemoryKind {
    Fact,
    Knowledge,
    Event,
}

/// Live/deleted filter for the author drilldown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorMemoryStateQuery {
    Live,
    Deleted,
    All,
}

/// Row returned by `Store::list_author_memories`. Carries the public
/// projection plus optional deletion metadata.
#[derive(Debug, Clone)]
pub struct AuthorMemoryRow {
    pub memory: klams_types::PublicMemory,
    pub state: AuthorMemoryStateOut,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub deleted_by: Option<klams_types::PublicAuthorRef>,
}

/// `live` or `deleted` state tag rendered in the API response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorMemoryStateOut {
    Live,
    Deleted,
}

// ---------------------------------------------------------------------
// Sprint 008 — `GET /v1/memories` + `event_search` shared types.

/// Query for [`Store::list_memories`]. `since`/`until` form an
/// inclusive-exclusive UTC window enforced by the API handler (see
/// [`klams_types::ApiConfig::memories_max_window_days`]). An empty
/// `kinds` vector means "all three kinds"; an empty `authors` vector
/// means "all authors".
#[derive(Debug, Clone)]
pub struct ListMemoriesQuery {
    pub since: chrono::DateTime<chrono::Utc>,
    pub until: chrono::DateTime<chrono::Utc>,
    pub kinds: Vec<MemoryKindFilter>,
    pub state: MemoryStateFilter,
    pub authors: Vec<Uuid>,
    pub limit: u32,
    pub cursor: Option<String>,
}

/// Which memory kinds to include in `list_memories`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKindFilter {
    Fact,
    Knowledge,
    Event,
}

/// Live/deleted filter for `list_memories`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStateFilter {
    Live,
    Deleted,
    All,
}

/// Row returned by [`Store::list_memories`].
#[derive(Debug, Clone)]
pub struct ListMemoriesRow {
    pub memory: klams_types::PublicMemory,
    pub state: MemoryStateOut,
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub deleted_by: Option<klams_types::PublicAuthorRef>,
}

/// `live` or `deleted` state tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStateOut {
    Live,
    Deleted,
}

/// Query for [`Store::event_search`]. An empty `author_ids` or
/// `categories` vector means "no filter on that dimension".
/// `payload_match` is an optional JSONB containment predicate
/// (`payload @> $payload_match`).
#[derive(Debug, Clone)]
pub struct EventSearchQuery {
    pub author_ids: Vec<Uuid>,
    pub categories: Vec<String>,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    pub payload_match: Option<serde_json::Value>,
    pub limit: u32,
    pub order: EventOrder,
    pub cursor: Option<String>,
}

/// Sort order for `event_search` (default `Desc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EventOrder {
    #[default]
    Desc,
    Asc,
}

/// Narrow trait the decay task uses to interact with the store
/// without pulling in the full `Store` surface. Implemented directly
/// by `PostgresStore` so tests can run without the Qdrant/TEI side.
#[async_trait]
pub trait DecayStore: Send + Sync + 'static {
    async fn select_decay_batch(
        &self,
        after_id: Option<Uuid>,
        limit: u32,
    ) -> StoreResult<Vec<DecayRow>>;
    async fn apply_decay_batch(&self, updates: &[(Uuid, f32)]) -> StoreResult<u64>;
    async fn apply_last_used_bumps(&self, ids: &[Uuid]) -> StoreResult<u64>;
}

/// Row returned by `select_decay_batch`. Held outside the trait so
/// `klams-core::decay` does not import any sql plumbing.
#[derive(Debug, Clone)]
pub struct DecayRow {
    pub id: Uuid,
    pub fact_type: FactType,
    pub age_seconds: f32,
}

// ---------------------------------------------------------------------------
// Sprint 005 (Phase 4) traits: hybrid retrieval + summary storage.
// ---------------------------------------------------------------------------

use klams_types::{ContextItem, EventSummary, RetrievalFilters, RetrievalSource, SummaryMechanism};
use time::Date;

/// A ranked row returned by a single retrieval source before fusion.
#[derive(Debug, Clone)]
pub struct RankedRow {
    pub source: RetrievalSource,
    pub id: Uuid,
    pub score: f32,
    /// Per-hit fusion weight (sprint 029, #644): RRF contributes
    /// `weight / (k + rank + 1)` instead of `1 / (k + rank + 1)`.
    /// `1.0` is the neutral value; provenance weighting raises it for
    /// curated hits. Weights scale a hit's *contribution*, never its
    /// rank within the source list.
    pub weight: f32,
    pub payload: serde_json::Value,
}

/// Narrow trait the `klams-core` `ContextBuilder` calls to fan out
/// across retrieval sources and receive ranked rows ready for fusion.
///
/// Implementations live in `klams-store::composite` (sprint 005 T028).
#[async_trait]
pub trait HybridStore: Send + Sync + 'static {
    /// Retrieve up to `per_source_top_k` rows from the given source
    /// for the query, honoring `filters`. Returns rows already
    /// scored by the source (no fusion happens here).
    async fn retrieve(
        &self,
        source: RetrievalSource,
        query: &str,
        filters: &RetrievalFilters,
        per_source_top_k: u32,
    ) -> StoreResult<Vec<RankedRow>>;
}

/// Narrow trait the summarization task uses to persist event
/// summaries to Postgres without depending on the full `Store`
/// surface. Implementations live in `klams-store::postgres`
/// (sprint 005 T037).
#[async_trait]
pub trait SummaryStore: Send + Sync + 'static {
    /// Upsert an event summary keyed by `(host, category, day_bucket)`.
    async fn upsert_event_summary(&self, summary: &EventSummary) -> StoreResult<()>;

    /// Mark all event summaries for the cluster as invalidated.
    async fn invalidate_event_summaries(
        &self,
        host: &str,
        category: &str,
        day_bucket: Date,
    ) -> StoreResult<u64>;

    /// Return the active (non-invalidated) summary for a cluster, if one
    /// exists. The retrieval path uses this to substitute a summary
    /// when raw events would blow the budget.
    async fn get_event_summary(
        &self,
        host: &str,
        category: &str,
        day_bucket: Date,
    ) -> StoreResult<Option<EventSummary>>;

    /// List active summaries that match the filters; used by the
    /// `ContextBuilder` to surface summaries through the retrieval
    /// path.
    async fn list_event_summaries(
        &self,
        filters: &RetrievalFilters,
        limit: u32,
    ) -> StoreResult<Vec<EventSummary>>;
}

/// Convenience helper: turn an `EventSummary` into a `ContextItem`
/// of `ItemKind::Summary` with no token accounting (the caller fills
/// in `tokens`).
#[must_use]
pub fn summary_to_context_item(summary: &EventSummary) -> ContextItem {
    use klams_types::ItemKind;
    ContextItem {
        kind: ItemKind::Summary,
        id: summary.id,
        score: 0.0,
        tokens: 0,
        payload: serde_json::json!({
            "host": summary.host,
            "category": summary.category,
            "day_bucket": summary.day_bucket.to_string(),
            "source_count": summary.source_count,
            "summary_text": summary.summary_text,
            "mechanism": summary.mechanism.as_str(),
        }),
        source_ids: Some(summary.source_ids.clone()),
    }
}

// Suppress dead-code warning for `SummaryMechanism` import in case
// no other module references it from this crate; we re-use it via
// `summary.mechanism`.
const _: Option<SummaryMechanism> = None;
