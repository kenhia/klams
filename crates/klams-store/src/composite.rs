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
