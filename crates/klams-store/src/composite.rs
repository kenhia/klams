//! Composite store wiring Postgres + Qdrant + TEI.

use crate::embeddings::TeiEmbedder;
use crate::postgres::PostgresStore;
use crate::qdrant::QdrantStore;
use crate::{DissentQuery, EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use async_trait::async_trait;
use klams_types::{
    AppendEvent, Dissent, Event, Fact, FactWriteOutcome, IndexKnowledge, KnowledgeItem, Source,
    UpsertFact,
};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CompositeStore {
    pub postgres: PostgresStore,
    pub qdrant: QdrantStore,
    pub embedder: TeiEmbedder,
    /// Optional outbound channel for `last_used_at` bumps. The
    /// `klams-core::DecayTask` drains the receiver. Reads that
    /// produce facts call `try_send` (drop-on-full).
    bump_tx: Option<mpsc::Sender<Uuid>>,
}

impl CompositeStore {
    pub fn new(postgres: PostgresStore, qdrant: QdrantStore, embedder: TeiEmbedder) -> Self {
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

    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        self.qdrant.get_knowledge(id).await
    }

    async fn delete_knowledge_by_source_file(&self, source_file: &str) -> StoreResult<u64> {
        self.qdrant.delete_by_source_file(source_file).await
    }

    async fn embed_query(&self, query: &str) -> StoreResult<Vec<f32>> {
        let start = std::time::Instant::now();
        let out = self.embedder.embed(query).await;
        metrics::histogram!("klams_embedding_latency_seconds")
            .record(start.elapsed().as_secs_f64());
        out
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
        let out: Vec<_> = rows.into_iter().map(authors_to_public).collect();
        let next_cursor = next.map(|(ts, id)| encode_author_cursor(ts, id));
        Ok((out, next_cursor))
    }

    async fn get_author_v1(&self, id: Uuid) -> StoreResult<Option<crate::AuthorWithCountsOut>> {
        Ok(self
            .postgres
            .get_author_with_counts(id)
            .await?
            .map(authors_to_public))
    }

    async fn list_author_memories(
        &self,
        q: crate::AuthorMemoriesQuery,
    ) -> StoreResult<(Vec<crate::AuthorMemoryRow>, Option<String>)> {
        list_author_memories_impl(self, q).await
    }
}

fn authors_to_public(row: crate::postgres::AuthorWithCounts) -> crate::AuthorWithCountsOut {
    crate::AuthorWithCountsOut {
        author: row.author,
        writes_facts: row.fact_count,
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
    let author_ref = PublicAuthorRef {
        agent_name: author.agent_name.clone(),
        model: author.model.clone(),
        repo: author.repo.clone(),
    };

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
            if s == "e" {
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
                    content: PublicMemoryContent::Knowledge {
                        text: item.text.clone(),
                        source_path: item.file.clone(),
                        repo: item.repo.clone(),
                    },
                    tags: item.tags.clone(),
                    author: author_ref.clone(),
                    created_at: offset_to_chrono(item.created_at),
                    updated_at: offset_to_chrono(item.updated_at),
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
            out.insert(
                *id,
                klams_types::PublicAuthorRef {
                    agent_name: a.agent_name,
                    model: a.model,
                    repo: a.repo,
                },
            );
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
