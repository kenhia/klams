//! Composite store wiring Postgres + Qdrant + a configured embedder.

use crate::embeddings::Embedder;
use crate::postgres::PostgresStore;
use crate::qdrant::QdrantStore;
use crate::{DissentQuery, EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use async_trait::async_trait;
use klams_types::{
    AppendEvent, Dissent, Event, Fact, FactWriteOutcome, IndexKnowledge, KnowledgeItem, Source,
    UpsertFact,
};
use std::sync::Arc;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CompositeStore {
    pub postgres: PostgresStore,
    pub qdrant: QdrantStore,
    pub embedder: Arc<dyn Embedder>,
    /// Optional outbound channel for `last_used_at` bumps. The
    /// `klams-core::DecayTask` drains the receiver. Reads that
    /// produce facts call `try_send` (drop-on-full).
    bump_tx: Option<mpsc::Sender<Uuid>>,
}

impl CompositeStore {
    pub fn new(postgres: PostgresStore, qdrant: QdrantStore, embedder: Arc<dyn Embedder>) -> Self {
        Self {
            postgres,
            qdrant,
            embedder,
            bump_tx: None,
        }
    }

    /// Wire a `LastUsedBumper` sender into the store so read paths
    /// flag returned facts for the decay task.
    #[must_use]
    pub fn with_bump_sender(mut self, tx: mpsc::Sender<Uuid>) -> Self {
        self.bump_tx = Some(tx);
        self
    }

    fn bump(&self, id: Uuid) {
        if let Some(tx) = &self.bump_tx {
            if tx.try_send(id).is_err() {
                metrics::counter!("klams_last_used_bumps_dropped_total").increment(1);
            }
        }
    }
}

#[async_trait]
impl Store for CompositeStore {
    async fn upsert_fact(&self, req: UpsertFact) -> StoreResult<Fact> {
        self.postgres.upsert_fact(req).await
    }

    async fn upsert_fact_v2(&self, req: UpsertFact) -> StoreResult<FactWriteOutcome> {
        self.postgres.upsert_fact_v2(req).await
    }

    async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        self.postgres.append_event(req).await
    }

    async fn index_knowledge(&self, req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        // Sprint 028 (#642): re-probe before embedding. The handler's
        // probe ran at enqueue time, so two queued jobs with the same
        // content (two files, or two hosts, in one scan window) would
        // both insert — with content-only identity that race is the
        // common case, not the corner. A hit here attaches the copy and
        // skips the embed entirely.
        if let Some(existing) = self
            .qdrant
            .find_knowledge_by_content_hash(&req.content_hash)
            .await?
        {
            if req.machine.is_some() || req.file.is_some() {
                self.qdrant
                    .attach_copy(
                        existing,
                        req.machine.as_deref(),
                        req.file.as_deref(),
                        req.repo.as_deref(),
                    )
                    .await?;
            }
            if let Some(item) = self.qdrant.get_knowledge(existing).await? {
                return Ok(item);
            }
        }
        let embedding = self.embedder.embed(&req.text).await?;
        self.qdrant.index_knowledge(req, embedding).await
    }

    async fn list_facts(&self, q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        let (facts, cursor) = self.postgres.list_facts(q).await?;
        for f in &facts {
            self.bump(f.id);
        }
        Ok((facts, cursor))
    }

    async fn list_events(&self, q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        self.postgres.list_events(q).await
    }

    async fn search_knowledge(
        &self,
        query_vector: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        self.qdrant.search_knowledge(query_vector, top_k).await
    }

    async fn search_text(
        &self,
        query: &str,
        top_k: u32,
    ) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        let (facts, events) = self.postgres.search_text(query, top_k).await?;
        for h in &facts {
            self.bump(h.id);
        }
        Ok((facts, events))
    }

    async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>> {
        self.qdrant.find_knowledge_by_content_hash(hash).await
    }

    async fn attach_knowledge_copy(
        &self,
        id: Uuid,
        machine: Option<&str>,
        file: Option<&str>,
        repo: Option<&str>,
    ) -> StoreResult<bool> {
        self.qdrant.attach_copy(id, machine, file, repo).await
    }

    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        self.qdrant.get_knowledge(id).await
    }

    async fn delete_knowledge_by_source_file(
        &self,
        source_file: &str,
        machine: Option<&str>,
    ) -> StoreResult<u64> {
        self.qdrant
            .delete_by_source_file(source_file, machine)
            .await
    }

    async fn embed_query(&self, query: &str) -> StoreResult<Vec<f32>> {
        let start = std::time::Instant::now();
        let out = self.embedder.embed(query).await;
        metrics::histogram!("klams_embedding_latency_seconds")
            .record(start.elapsed().as_secs_f64());
        out
    }

    /// Sprint 027 (#420): ask the embedder for the *exact* token count
    /// rather than estimating from character counts. Measured against
    /// the live model, no character ratio is both safe and useful —
    /// punctuation-dense text hits the ceiling at ~525 characters while
    /// base64 survives past 20,000 — so the tokenizer is the only
    /// honest arbiter, and TEI's `/tokenize` provides it without a model
    /// forward pass.
    async fn check_embed_size(
        &self,
        text: &str,
        limit: klams_types::EmbedLimit,
    ) -> StoreResult<()> {
        let counts = self.embedder.count_tokens(&[text]).await?;
        let tokens = counts.first().copied().unwrap_or(0);
        if tokens <= limit.max_input_tokens() {
            return Ok(());
        }
        Err(StoreError::PayloadTooLarge {
            oversize: crate::embeddings::oversize_from_exact_count(text, tokens, limit),
            detail: "exact token count from the embedding model".into(),
        })
    }

    async fn health_postgres(&self) -> StoreResult<()> {
        self.postgres.health().await
    }
    async fn health_qdrant(&self) -> StoreResult<()> {
        self.qdrant.health().await
    }
    async fn health_embedder(&self) -> StoreResult<()> {
        self.embedder.health().await
    }

    async fn list_dissents(&self, q: DissentQuery) -> StoreResult<(Vec<Dissent>, Option<String>)> {
        self.postgres.list_dissents(q).await
    }
    async fn get_dissent(&self, id: Uuid) -> StoreResult<Option<Dissent>> {
        self.postgres.get_dissent(id).await
    }
    async fn promote_dissent(
        &self,
        dissent_id: Uuid,
        caller_source: Source,
        expected_version: i32,
    ) -> StoreResult<Fact> {
        self.postgres
            .promote_dissent(dissent_id, caller_source, expected_version)
            .await
    }
    async fn discard_dissent(
        &self,
        dissent_id: Uuid,
        caller_source: Source,
    ) -> StoreResult<Dissent> {
        self.postgres
            .discard_dissent(dissent_id, caller_source)
            .await
    }

    async fn list_authors_v1(
        &self,
        q: crate::AuthorListQuery,
    ) -> StoreResult<(Vec<crate::AuthorWithCountsOut>, Option<String>)> {
        let cursor = q.cursor.as_deref().and_then(decode_author_cursor);
        let limit = if q.limit == 0 { 50 } else { q.limit };
        let (rows, next) = self
            .postgres
            .list_authors_with_counts_filtered(limit, q.since, q.agent_name.as_deref(), cursor)
            .await?;
        let mut out: Vec<_> = rows.into_iter().map(authors_to_public).collect();
        // Sprint 009 (kwi #32 followup, T049): populate Qdrant knowledge
        // counts in parallel. The `author_id` payload index (T048)
        // makes each filtered count O(matches) rather than a full scan,
        // so 50 parallel calls stay well under the SC-006 1 s budget.
        // Fail-soft per author: a Qdrant error leaves the count at 0.
        let handles: Vec<_> = out
            .iter()
            .map(|a| {
                let qd = self.qdrant.clone();
                let id = a.author.id;
                tokio::spawn(async move { (id, qd.count_live_knowledge_by_author(id).await) })
            })
            .collect();
        let mut counts: std::collections::HashMap<Uuid, i64> =
            std::collections::HashMap::with_capacity(handles.len());
        for h in handles {
            if let Ok((id, Ok(n))) = h.await {
                counts.insert(id, i64::try_from(n).unwrap_or(i64::MAX));
            }
        }
        for a in &mut out {
            if let Some(&n) = counts.get(&a.author.id) {
                a.writes_knowledge = n;
            }
        }
        let next_cursor = next.map(|(ts, id)| encode_author_cursor(ts, id));
        Ok((out, next_cursor))
    }

    async fn get_author_v1(&self, id: Uuid) -> StoreResult<Option<crate::AuthorWithCountsOut>> {
        let Some(row) = self.postgres.get_author_with_counts(id).await? else {
            return Ok(None);
        };
        let mut out = authors_to_public(row);
        // Populate knowledge count from Qdrant; fail-soft to 0 on
        // backend errors so the detail page still renders.
        out.writes_knowledge = match self.qdrant.count_live_knowledge_by_author(id).await {
            Ok(n) => i64::try_from(n).unwrap_or(i64::MAX),
            Err(_) => 0,
        };
        Ok(Some(out))
    }

    async fn list_author_memories(
        &self,
        q: crate::AuthorMemoriesQuery,
    ) -> StoreResult<(Vec<crate::AuthorMemoryRow>, Option<String>)> {
        list_author_memories_impl(self, q).await
    }

    async fn list_memories(
        &self,
        q: crate::ListMemoriesQuery,
    ) -> StoreResult<(Vec<crate::ListMemoriesRow>, Option<String>)> {
        list_memories_impl(self, q).await
    }

    async fn event_search(
        &self,
        q: crate::EventSearchQuery,
    ) -> StoreResult<(Vec<klams_types::PublicMemory>, Option<String>)> {
        event_search_impl(self, q).await
    }

    async fn touch_author_last_seen_at(&self, id: Uuid) -> StoreResult<u64> {
        self.postgres.touch_author_last_seen_at(id).await
    }
}

fn authors_to_public(row: crate::postgres::AuthorWithCounts) -> crate::AuthorWithCountsOut {
    crate::AuthorWithCountsOut {
        author: row.author,
        writes_facts: row.fact_count,
        writes_knowledge: 0,
        events: row.event_count,
        soft_deletes_authored: row.soft_deletes_authored,
        restores_received: row.restores_received,
    }
}

fn offset_to_chrono(ts: time::OffsetDateTime) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(
        i64::try_from(ts.unix_timestamp_nanos()).unwrap_or(0),
    )
}

fn encode_author_cursor(ts: chrono::DateTime<chrono::Utc>, id: Uuid) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    URL_SAFE_NO_PAD.encode(format!("{}:{}", ts.timestamp_nanos_opt().unwrap_or(0), id))
}

fn decode_author_cursor(raw: &str) -> Option<(chrono::DateTime<chrono::Utc>, Uuid)> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let s = std::str::from_utf8(&bytes).ok()?;
    let (ns, id) = s.rsplit_once(':')?;
    let ns: i64 = ns.parse().ok()?;
    let ts = chrono::DateTime::<chrono::Utc>::from_timestamp_nanos(ns);
    let id = Uuid::parse_str(id).ok()?;
    Some((ts, id))
}

fn encode_memory_cursor(section: &str, ts_nanos: i128, id: Uuid) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    URL_SAFE_NO_PAD.encode(format!("{section}:{ts_nanos}:{id}"))
}

fn decode_memory_cursor(raw: &str) -> Option<(String, i128, Uuid)> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let s = std::str::from_utf8(&bytes).ok()?;
    let mut parts = s.splitn(3, ':');
    let section = parts.next()?.to_string();
    let ts: i128 = parts.next()?.parse().ok()?;
    let id = Uuid::parse_str(parts.next()?).ok()?;
    Some((section, ts, id))
}

#[allow(clippy::too_many_lines)]
async fn list_author_memories_impl(
    composite: &CompositeStore,
    q: crate::AuthorMemoriesQuery,
) -> StoreResult<(Vec<crate::AuthorMemoryRow>, Option<String>)> {
    use crate::{AuthorMemoryKind, AuthorMemoryStateOut, AuthorMemoryStateQuery};
    use klams_types::{PublicAuthorRef, PublicMemory, PublicMemoryContent};

    // Resolve the author's own PublicAuthorRef (used on every row).
    let author = composite
        .postgres
        .get_author_by_id(q.author_id)
        .await?
        .ok_or_else(|| StoreError::Other(format!("author {} not found", q.author_id)))?;
    let author_ref = PublicAuthorRef::from_record(&author);

    let limit = if q.limit == 0 { 50 } else { q.limit };
    let pg_state = match q.state {
        AuthorMemoryStateQuery::Live => crate::postgres::AuthorMemoryState::Live,
        AuthorMemoryStateQuery::Deleted => crate::postgres::AuthorMemoryState::Deleted,
        AuthorMemoryStateQuery::All => crate::postgres::AuthorMemoryState::All,
    };
    let qd_state = match q.state {
        AuthorMemoryStateQuery::Live => crate::qdrant::AuthorMemoryStateFilter::Live,
        AuthorMemoryStateQuery::Deleted => crate::qdrant::AuthorMemoryStateFilter::Deleted,
        AuthorMemoryStateQuery::All => crate::qdrant::AuthorMemoryStateFilter::All,
    };

    let cursor = q.cursor.as_deref().and_then(decode_memory_cursor);
    let section = cursor
        .as_ref()
        .map_or_else(|| "f".to_string(), |c| c.0.clone());

    let want_facts = q.kinds.is_empty() || q.kinds.contains(&AuthorMemoryKind::Fact);
    let want_events = q.kinds.is_empty() || q.kinds.contains(&AuthorMemoryKind::Event);
    let want_knowledge = q.kinds.is_empty() || q.kinds.contains(&AuthorMemoryKind::Knowledge);

    // -- facts page --
    if section == "f" && want_facts {
        let pg_cursor = cursor.as_ref().and_then(|(s, ns, id)| {
            if s == "f" {
                Some((
                    time::OffsetDateTime::from_unix_timestamp_nanos(*ns).ok()?,
                    *id,
                ))
            } else {
                None
            }
        });
        let (rows, next) = composite
            .postgres
            .list_facts_by_author(q.author_id, pg_state, limit, pg_cursor)
            .await?;
        let len = rows.len();
        let mut deleter_ids = Vec::new();
        for (_, _, d) in &rows {
            if let Some(d) = d {
                deleter_ids.push(*d);
            }
        }
        let deleters = bulk_fetch_authors(composite, &deleter_ids).await;
        let out: Vec<_> = rows
            .into_iter()
            .map(|(f, deleted_at, deleter)| {
                let state = if deleted_at.is_some() {
                    AuthorMemoryStateOut::Deleted
                } else {
                    AuthorMemoryStateOut::Live
                };
                let mem = PublicMemory {
                    id: f.id,
                    content: PublicMemoryContent::Fact {
                        fact_type: f.fact_type.as_str().to_string(),
                        payload: f.payload.clone(),
                    },
                    tags: Vec::new(),
                    author: author_ref.clone(),
                    created_at: offset_to_chrono(f.created_at),
                    updated_at: offset_to_chrono(f.updated_at),
                    deleted_at: None,
                    deleted_by_author_id: None,
                };
                crate::AuthorMemoryRow {
                    memory: mem,
                    state,
                    deleted_at: deleted_at.map(offset_to_chrono),
                    deleted_by: deleter.and_then(|d| deleters.get(&d).cloned()),
                }
            })
            .collect();
        let next_cursor = if let Some((ts, id)) = next {
            Some(encode_memory_cursor("f", ts.unix_timestamp_nanos(), id))
        } else if want_events {
            Some(encode_memory_cursor("e", 0, Uuid::nil()))
        } else if want_knowledge {
            Some(encode_memory_cursor("k", 0, Uuid::nil()))
        } else {
            None
        };
        // If the page is empty but we have more sections to try, fall through.
        if !out.is_empty() || len > 0 {
            return Ok((out, next_cursor));
        }
    }

    if (section == "f" || section == "e") && want_events {
        let pg_cursor = cursor.as_ref().and_then(|(s, ns, id)| {
            // Section-handoff sentinel: (ns=0, id=nil) means "start of
            // events section", not "after epoch-0/nil" — skip the cursor.
            if s == "e" && !(*ns == 0 && *id == Uuid::nil()) {
                Some((
                    time::OffsetDateTime::from_unix_timestamp_nanos(*ns).ok()?,
                    *id,
                ))
            } else {
                None
            }
        });
        let (rows, next) = composite
            .postgres
            .list_events_by_author(q.author_id, limit, pg_cursor)
            .await?;
        let out: Vec<_> = rows
            .into_iter()
            .map(|e| {
                let mem = PublicMemory {
                    id: e.id,
                    content: PublicMemoryContent::Event {
                        category: e.category.clone(),
                        payload: e.payload.clone(),
                        task_id: e.task_id,
                    },
                    tags: Vec::new(),
                    author: author_ref.clone(),
                    created_at: offset_to_chrono(e.created_at),
                    updated_at: offset_to_chrono(e.created_at),
                    deleted_at: None,
                    deleted_by_author_id: None,
                };
                crate::AuthorMemoryRow {
                    memory: mem,
                    state: AuthorMemoryStateOut::Live,
                    deleted_at: None,
                    deleted_by: None,
                }
            })
            .collect();
        let next_cursor = if let Some((ts, id)) = next {
            Some(encode_memory_cursor("e", ts.unix_timestamp_nanos(), id))
        } else if want_knowledge {
            Some(encode_memory_cursor("k", 0, Uuid::nil()))
        } else {
            None
        };
        if !out.is_empty() {
            return Ok((out, next_cursor));
        }
    }

    if want_knowledge {
        let qd_cursor = cursor.as_ref().and_then(|(s, _, id)| {
            if s == "k" && *id != Uuid::nil() {
                Some(*id)
            } else {
                None
            }
        });
        let (rows, next) = composite
            .qdrant
            .list_knowledge_by_author(q.author_id, qd_state, limit, qd_cursor)
            .await?;
        let out: Vec<_> = rows
            .into_iter()
            .map(|(item, deleted_at, _deleter)| {
                let state = if deleted_at.is_some() {
                    AuthorMemoryStateOut::Deleted
                } else {
                    AuthorMemoryStateOut::Live
                };
                let mem = PublicMemory {
                    id: item.id,
                    content: PublicMemoryContent::knowledge_from(&item),
                    tags: item.tags.clone(),
                    author: author_ref.clone(),
                    created_at: offset_to_chrono(item.created_at),
                    updated_at: offset_to_chrono(item.updated_at),
                    deleted_at: None,
                    deleted_by_author_id: None,
                };
                crate::AuthorMemoryRow {
                    memory: mem,
                    state,
                    deleted_at: deleted_at.map(offset_to_chrono),
                    // Knowledge deleter resolution requires another author lookup;
                    // skipped in v1 to keep the scroll cheap.
                    deleted_by: None,
                }
            })
            .collect();
        let next_cursor = next.map(|id| encode_memory_cursor("k", 0, id));
        return Ok((out, next_cursor));
    }

    Ok((Vec::new(), None))
}

async fn bulk_fetch_authors(
    composite: &CompositeStore,
    ids: &[Uuid],
) -> std::collections::HashMap<Uuid, klams_types::PublicAuthorRef> {
    let mut out = std::collections::HashMap::new();
    for id in ids {
        if out.contains_key(id) {
            continue;
        }
        if let Ok(Some(a)) = composite.postgres.get_author_by_id(*id).await {
            out.insert(*id, klams_types::PublicAuthorRef::from_record(&a));
        }
    }
    out
}

#[async_trait]
impl crate::DecayStore for CompositeStore {
    async fn select_decay_batch(
        &self,
        after_id: Option<Uuid>,
        limit: u32,
    ) -> StoreResult<Vec<crate::DecayRow>> {
        self.postgres.select_decay_batch(after_id, limit).await
    }
    async fn apply_decay_batch(&self, updates: &[(Uuid, f32)]) -> StoreResult<u64> {
        self.postgres.apply_decay_batch(updates).await
    }
    async fn apply_last_used_bumps(&self, ids: &[Uuid]) -> StoreResult<u64> {
        self.postgres.apply_last_used_bumps(ids).await
    }
}

// Sprint 005 T037 — delegate the SummaryStore surface to Postgres.
#[async_trait]
impl crate::SummaryStore for CompositeStore {
    async fn upsert_event_summary(&self, summary: &klams_types::EventSummary) -> StoreResult<()> {
        crate::SummaryStore::upsert_event_summary(&self.postgres, summary).await
    }
    async fn invalidate_event_summaries(
        &self,
        host: &str,
        category: &str,
        day_bucket: time::Date,
    ) -> StoreResult<u64> {
        crate::SummaryStore::invalidate_event_summaries(&self.postgres, host, category, day_bucket)
            .await
    }
    async fn get_event_summary(
        &self,
        host: &str,
        category: &str,
        day_bucket: time::Date,
    ) -> StoreResult<Option<klams_types::EventSummary>> {
        crate::SummaryStore::get_event_summary(&self.postgres, host, category, day_bucket).await
    }
    async fn list_event_summaries(
        &self,
        filters: &klams_types::RetrievalFilters,
        limit: u32,
    ) -> StoreResult<Vec<klams_types::EventSummary>> {
        crate::SummaryStore::list_event_summaries(&self.postgres, filters, limit).await
    }
}

// Suppress unused-import warning when not all variants are used yet.
fn _dummy_error_ref() -> Option<StoreError> {
    None
}

// ---------------------------------------------------------------------
// Sprint 008 — `GET /v1/memories` and `event_search` impls.

fn unknown_author_ref() -> klams_types::PublicAuthorRef {
    klams_types::PublicAuthorRef::unknown()
}

/// Encode the unified newest-first cursor for `list_memories` (#54): the
/// `(created_at_nanos, id)` keyset of the last row returned. Format
/// `base64_url("ns:uuid")`; supersedes the old per-section `section:ns:uuid`
/// cursor, whose stale form is still tolerated on decode across a deploy.
fn encode_merged_cursor(ts_nanos: i128, id: Uuid) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    URL_SAFE_NO_PAD.encode(format!("{ts_nanos}:{id}"))
}

fn decode_merged_cursor(raw: &str) -> Option<(i128, Uuid)> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    let s = std::str::from_utf8(&bytes).ok()?;
    // Accept the current `ns:uuid` form and, for resilience across a deploy, the
    // legacy `section:ns:uuid` form (the section tag is ignored).
    let parts: Vec<&str> = s.split(':').collect();
    let (ns, id) = match parts.as_slice() {
        [ns, id] => (*ns, *id),
        [_section, ns, id] => (*ns, *id),
        _ => return None,
    };
    Some((ns.parse().ok()?, Uuid::parse_str(id).ok()?))
}

/// Merge already-per-source newest-first rows into one global newest-first page.
/// Each source has supplied up to `limit` rows ordered `created_at DESC, id DESC`
/// after the *same* keyset, so the global top-`limit` is guaranteed to lie within
/// the union: sort, take `limit`, and return the `(created_at_nanos, id)` of the
/// last emitted row as the next keyset — only when a further page may exist
/// (either the union was truncated, or some source signalled saturation).
fn take_merged_page<T>(
    mut cands: Vec<(i128, Uuid, T)>,
    limit: usize,
    any_saturated: bool,
) -> (Vec<T>, Option<(i128, Uuid)>) {
    cands.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let truncated = cands.len() > limit;
    let emit = limit.min(cands.len());
    let next = if (truncated || any_saturated) && emit == limit {
        cands.get(emit - 1).map(|(ns, id, _)| (*ns, *id))
    } else {
        None
    };
    let out = cands
        .into_iter()
        .take(emit)
        .map(|(_, _, row)| row)
        .collect();
    (out, next)
}

#[allow(clippy::too_many_lines)]
async fn list_memories_impl(
    composite: &CompositeStore,
    q: crate::ListMemoriesQuery,
) -> StoreResult<(Vec<crate::ListMemoriesRow>, Option<String>)> {
    use crate::{MemoryKindFilter, MemoryStateFilter, MemoryStateOut};
    use klams_types::{PublicMemory, PublicMemoryContent};

    let limit = if q.limit == 0 { 50 } else { q.limit };
    let pg_state = match q.state {
        MemoryStateFilter::Live => crate::postgres::AuthorMemoryState::Live,
        MemoryStateFilter::Deleted => crate::postgres::AuthorMemoryState::Deleted,
        MemoryStateFilter::All => crate::postgres::AuthorMemoryState::All,
    };
    let qd_state = match q.state {
        MemoryStateFilter::Live => crate::qdrant::AuthorMemoryStateFilter::Live,
        MemoryStateFilter::Deleted => crate::qdrant::AuthorMemoryStateFilter::Deleted,
        MemoryStateFilter::All => crate::qdrant::AuthorMemoryStateFilter::All,
    };

    // Unified newest-first keyset across all three kinds (#54). The previous
    // implementation emitted whole sections in a fixed order (all facts, then
    // all events, then all knowledge), so a brand-new knowledge item sorted
    // below every fact and event, and the knowledge section itself came back
    // oldest-first (Qdrant point-id order). Now each kind is paged
    // `created_at DESC` after the same `(created_at, id)` keyset and merged.
    let keyset: Option<(time::OffsetDateTime, Uuid)> = q
        .cursor
        .as_deref()
        .and_then(decode_merged_cursor)
        .and_then(|(ns, id)| {
            Some((
                time::OffsetDateTime::from_unix_timestamp_nanos(ns).ok()?,
                id,
            ))
        });

    let want_facts = q.kinds.is_empty() || q.kinds.contains(&MemoryKindFilter::Fact);
    let want_events = q.kinds.is_empty() || q.kinds.contains(&MemoryKindFilter::Event);
    let want_knowledge = q.kinds.is_empty() || q.kinds.contains(&MemoryKindFilter::Knowledge);

    let mut candidates: Vec<crate::ListMemoriesRow> = Vec::new();
    // A source "saturated" (returned a full page) may have more rows just past
    // this page, so the merge must offer a next cursor even if the union fit.
    let mut saturated = false;

    // -- facts (Postgres, created_at DESC keyset) --
    if want_facts {
        let (rows, next) = composite
            .postgres
            .list_memories_facts_page(&q.authors, pg_state, q.since, q.until, limit, keyset)
            .await?;
        saturated |= next.is_some();
        let mut needed: Vec<Uuid> = Vec::with_capacity(rows.len() * 2);
        for (_, aid, _, d) in &rows {
            needed.push(*aid);
            if let Some(d) = d {
                needed.push(*d);
            }
        }
        let authors = bulk_fetch_authors(composite, &needed).await;
        for (f, author_id, deleted_at, deleter) in rows {
            let state = if deleted_at.is_some() {
                MemoryStateOut::Deleted
            } else {
                MemoryStateOut::Live
            };
            let author_ref = authors
                .get(&author_id)
                .cloned()
                .unwrap_or_else(unknown_author_ref);
            let mem = PublicMemory {
                id: f.id,
                content: PublicMemoryContent::Fact {
                    fact_type: f.fact_type.as_str().to_string(),
                    payload: f.payload.clone(),
                },
                tags: Vec::new(),
                author: author_ref,
                created_at: offset_to_chrono(f.created_at),
                updated_at: offset_to_chrono(f.updated_at),
                deleted_at: deleted_at.map(offset_to_chrono),
                deleted_by_author_id: deleter,
            };
            candidates.push(crate::ListMemoriesRow {
                memory: mem,
                state,
                deleted_at: deleted_at.map(offset_to_chrono),
                deleted_by: deleter.and_then(|d| authors.get(&d).cloned()),
            });
        }
    }

    // -- events (Postgres, created_at DESC keyset) --
    if want_events {
        let (rows, next) = composite
            .postgres
            .list_memories_events_page(&q.authors, q.since, q.until, limit, keyset)
            .await?;
        saturated |= next.is_some();
        let needed: Vec<Uuid> = rows.iter().map(|(_, aid)| *aid).collect();
        let authors = bulk_fetch_authors(composite, &needed).await;
        for (e, author_id) in rows {
            let author_ref = authors
                .get(&author_id)
                .cloned()
                .unwrap_or_else(unknown_author_ref);
            let mem = PublicMemory {
                id: e.id,
                content: PublicMemoryContent::Event {
                    category: e.category.clone(),
                    payload: e.payload.clone(),
                    task_id: e.task_id,
                },
                tags: Vec::new(),
                author: author_ref,
                created_at: offset_to_chrono(e.created_at),
                updated_at: offset_to_chrono(e.created_at),
                deleted_at: None,
                deleted_by_author_id: None,
            };
            candidates.push(crate::ListMemoriesRow {
                memory: mem,
                state: MemoryStateOut::Live,
                deleted_at: None,
                deleted_by: None,
            });
        }
    }

    // -- knowledge (Qdrant, created_at DESC via datetime-index order_by) --
    if want_knowledge {
        let (rows, next) = composite
            .qdrant
            .list_memories_knowledge_page(&q.authors, qd_state, q.since, q.until, limit, keyset)
            .await?;
        saturated |= next.is_some();
        let needed: Vec<Uuid> = rows.iter().map(|(_, aid, _, _)| *aid).collect();
        let authors = bulk_fetch_authors(composite, &needed).await;
        for (item, author_id, deleted_at, deleter) in rows {
            let state = if deleted_at.is_some() {
                MemoryStateOut::Deleted
            } else {
                MemoryStateOut::Live
            };
            let author_ref = authors
                .get(&author_id)
                .cloned()
                .unwrap_or_else(unknown_author_ref);
            let mem = PublicMemory {
                id: item.id,
                content: PublicMemoryContent::knowledge_from(&item),
                tags: item.tags.clone(),
                author: author_ref,
                created_at: offset_to_chrono(item.created_at),
                updated_at: offset_to_chrono(item.updated_at),
                deleted_at: deleted_at.map(offset_to_chrono),
                deleted_by_author_id: deleter,
            };
            candidates.push(crate::ListMemoriesRow {
                memory: mem,
                state,
                deleted_at: deleted_at.map(offset_to_chrono),
                deleted_by: None,
            });
        }
    }

    // Global newest-first merge across the three kinds.
    let keyed: Vec<(i128, Uuid, crate::ListMemoriesRow)> = candidates
        .into_iter()
        .map(|row| {
            let ns = i128::from(row.memory.created_at.timestamp_nanos_opt().unwrap_or(0));
            let id = row.memory.id;
            (ns, id, row)
        })
        .collect();
    let (out, next_key) = take_merged_page(keyed, limit as usize, saturated);
    let next_cursor = next_key.map(|(ns, id)| encode_merged_cursor(ns, id));
    Ok((out, next_cursor))
}

async fn event_search_impl(
    composite: &CompositeStore,
    q: crate::EventSearchQuery,
) -> StoreResult<(Vec<klams_types::PublicMemory>, Option<String>)> {
    use klams_types::{PublicMemory, PublicMemoryContent};

    let limit = if q.limit == 0 { 50 } else { q.limit };
    let ascending = matches!(q.order, crate::EventOrder::Asc);
    let pg_cursor = q.cursor.as_deref().and_then(|raw| {
        decode_memory_cursor(raw).and_then(|(s, ns, id)| {
            if s == "e" {
                Some((
                    time::OffsetDateTime::from_unix_timestamp_nanos(ns).ok()?,
                    id,
                ))
            } else {
                None
            }
        })
    });
    let (rows, next) = composite
        .postgres
        .event_search_page(
            &q.author_ids,
            &q.categories,
            q.since,
            q.until,
            q.payload_match.as_ref(),
            limit,
            ascending,
            pg_cursor,
        )
        .await?;
    let needed: Vec<Uuid> = rows.iter().map(|(_, aid)| *aid).collect();
    let authors = bulk_fetch_authors(composite, &needed).await;
    let out: Vec<_> = rows
        .into_iter()
        .map(|(e, author_id)| {
            let author_ref = authors
                .get(&author_id)
                .cloned()
                .unwrap_or_else(unknown_author_ref);
            PublicMemory {
                id: e.id,
                content: PublicMemoryContent::Event {
                    category: e.category.clone(),
                    payload: e.payload.clone(),
                    task_id: e.task_id,
                },
                tags: Vec::new(),
                author: author_ref,
                created_at: offset_to_chrono(e.created_at),
                updated_at: offset_to_chrono(e.created_at),
                deleted_at: None,
                deleted_by_author_id: None,
            }
        })
        .collect();
    let next_cursor = next.map(|(ts, id)| encode_memory_cursor("e", ts.unix_timestamp_nanos(), id));
    Ok((out, next_cursor))
}

#[cfg(test)]
mod cursor_tests {
    use super::{decode_memory_cursor, encode_memory_cursor};
    use uuid::Uuid;

    /// Sprint 008 T014 — round-trip the v2 cursor format used by
    /// `list_memories` and `event_search`. Confirms the on-the-wire
    /// shape (`base64_url(section:ts_nanos:uuid)`) is stable across
    /// the three section tags.
    #[test]
    fn round_trips_each_section() {
        let id = Uuid::parse_str("019470e2-fc4a-7000-8000-000000000001").unwrap();
        for section in ["f", "k", "e"] {
            let ts_nanos: i128 = 1_733_000_000_000_000_000;
            let cursor = encode_memory_cursor(section, ts_nanos, id);
            let (got_section, got_ts, got_id) = decode_memory_cursor(&cursor).unwrap();
            assert_eq!(got_section, section);
            assert_eq!(got_ts, ts_nanos);
            assert_eq!(got_id, id);
        }
    }

    #[test]
    fn rejects_garbage_input() {
        assert!(decode_memory_cursor("!!!not-base64!!!").is_none());
        assert!(decode_memory_cursor("").is_none());
    }
}

#[cfg(test)]
mod merge_tests {
    use super::{decode_merged_cursor, encode_merged_cursor, take_merged_page};
    use uuid::Uuid;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// #54 — the unified `(ns:uuid)` cursor round-trips, and a legacy
    /// per-section `section:ns:uuid` cursor still decodes (section ignored) so
    /// in-flight cursors survive a deploy.
    #[test]
    fn merged_cursor_round_trip_and_legacy() {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine as _;

        let c = encode_merged_cursor(1_733_000_000_000_000_000, id(7));
        assert_eq!(
            decode_merged_cursor(&c),
            Some((1_733_000_000_000_000_000, id(7)))
        );
        // legacy 3-part form (base64 of "k:123:<uuid>")
        let legacy = URL_SAFE_NO_PAD.encode(format!("k:123:{}", id(9)));
        assert_eq!(decode_merged_cursor(&legacy), Some((123, id(9))));
        assert!(decode_merged_cursor("!!!").is_none());
    }

    /// The merge interleaves the three per-source (already DESC) inputs into one
    /// globally newest-first page and hands back the last row's keyset.
    #[test]
    fn merges_kinds_globally_newest_first() {
        // facts, events, knowledge each arrive newest-first from their store.
        let facts = vec![(300i128, id(1), 'A'), (100, id(2), 'D')];
        let events = vec![(250, id(3), 'B')];
        let knowledge = vec![(280, id(4), 'C'), (120, id(5), 'E')];
        let cands: Vec<_> = facts.into_iter().chain(events).chain(knowledge).collect();
        let (out, next) = take_merged_page(cands, 3, true);
        assert_eq!(out, vec!['A', 'C', 'B']); // 300, 280, 250
        assert_eq!(next, Some((250, id(3)))); // last emitted -> next keyset
    }

    /// Ties on `created_at` break by id DESC, consistently across sources.
    #[test]
    fn ties_break_by_id_desc() {
        let cands = vec![(100i128, id(1), 'x'), (100, id(9), 'y'), (100, id(5), 'z')];
        let (out, _next) = take_merged_page(cands, 3, false);
        assert_eq!(out, vec!['y', 'z', 'x']); // id 9 > 5 > 1
    }

    /// A fully-drained union (nothing saturated, fewer than `limit`) ends the
    /// feed with no next cursor.
    #[test]
    fn drained_union_has_no_next() {
        let cands = vec![(300i128, id(1), 'A'), (200, id(2), 'B')];
        let (out, next) = take_merged_page(cands, 10, false);
        assert_eq!(out, vec!['A', 'B']);
        assert_eq!(next, None);
    }

    /// A full page with a saturated source keeps paging even when the union
    /// exactly equals `limit`.
    #[test]
    fn saturated_source_offers_next_at_exact_limit() {
        let cands = vec![(300i128, id(1), 'A'), (200, id(2), 'B')];
        let (out, next) = take_merged_page(cands, 2, true);
        assert_eq!(out, vec!['A', 'B']);
        assert_eq!(next, Some((200, id(2))));
    }
}
