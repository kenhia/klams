//! Storage adapters: Postgres (facts/events), Qdrant (knowledge),
//! and TEI embeddings.
//!
//! Exposes a `Store` trait that the worker pool drives, plus a
//! `CompositeStore` that wires the three backends together.

use async_trait::async_trait;
use klams_types::{
    AppendEvent, Event, Fact, FactType, IndexKnowledge, KnowledgeItem, Source, UpsertFact,
};
use std::error::Error;
use std::fmt;
use time::OffsetDateTime;
use uuid::Uuid;

pub mod composite;
pub mod embeddings;
pub mod postgres;
pub mod qdrant;

pub use composite::CompositeStore;
pub use embeddings::TeiEmbedder;
pub use postgres::PostgresStore;
pub use qdrant::QdrantStore;

#[derive(Debug)]
pub enum StoreError {
    Backend(String),
    Conflict(String),
    Embedding(String),
    Other(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Backend(m) => write!(f, "store backend error: {m}"),
            StoreError::Conflict(m) => write!(f, "store conflict: {m}"),
            StoreError::Embedding(m) => write!(f, "embedding error: {m}"),
            StoreError::Other(m) => write!(f, "store error: {m}"),
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
    pub category: Option<String>,
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

/// Single trait the worker pool uses for all persistence.
#[async_trait]
pub trait Store: Send + Sync + 'static {
    async fn upsert_fact(&self, req: UpsertFact) -> StoreResult<Fact>;
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
    async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>>;
    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>>;
    /// Embed a free-text query into a vector compatible with
    /// [`search_knowledge`]. Implementations backed by Qdrant + TEI
    /// delegate to the embedder; mock stores can return a zero vector.
    async fn embed_query(&self, query: &str) -> StoreResult<Vec<f32>>;

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
}
