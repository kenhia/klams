//! Composite store wiring Postgres + Qdrant + TEI.

use crate::embeddings::TeiEmbedder;
use crate::postgres::PostgresStore;
use crate::qdrant::QdrantStore;
use crate::{EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use async_trait::async_trait;
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, UpsertFact};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CompositeStore {
    pub postgres: PostgresStore,
    pub qdrant: QdrantStore,
    pub embedder: TeiEmbedder,
}

impl CompositeStore {
    pub fn new(postgres: PostgresStore, qdrant: QdrantStore, embedder: TeiEmbedder) -> Self {
        Self {
            postgres,
            qdrant,
            embedder,
        }
    }
}

#[async_trait]
impl Store for CompositeStore {
    async fn upsert_fact(&self, req: UpsertFact) -> StoreResult<Fact> {
        self.postgres.upsert_fact(req).await
    }

    async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        self.postgres.append_event(req).await
    }

    async fn index_knowledge(&self, req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        let embedding = self.embedder.embed(&req.text).await?;
        self.qdrant.index_knowledge(req, embedding).await
    }

    async fn list_facts(&self, q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        self.postgres.list_facts(q).await
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
        self.postgres.search_text(query, top_k).await
    }

    async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>> {
        self.qdrant.find_knowledge_by_content_hash(hash).await
    }

    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        self.qdrant.get_knowledge(id).await
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
}

// Suppress unused-import warning when not all variants are used yet.
fn _dummy_error_ref() -> Option<StoreError> {
    None
}
