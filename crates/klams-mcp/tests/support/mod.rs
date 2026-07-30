//! An in-memory `Store` for testing MCP tool handlers without a stack.
//!
//! Sprint 031 (#646). Seventeen test files live in this crate; thirteen
//! were empty `#[ignore]`d stubs whose entire body was a comment
//! pointing at a `klams-service` integration test. They were not
//! skipped tests — there was nothing to skip. `McpState` held a
//! concrete `Arc<CompositeStore>`, so exercising a handler meant
//! standing up Postgres, Qdrant and TEI, and nobody was going to do
//! that for a unit-level assertion.
//!
//! #645 made `McpState` generic over `Store`, which is what makes this
//! file possible.
//!
//! # What this is and is not
//!
//! [`MemStore`] holds real state — authors, facts, knowledge, events,
//! soft-delete flags — so handler behaviour can be asserted end to end:
//! argument validation, scope and ownership checks, error codes, the
//! projection shape an agent actually receives.
//!
//! It is NOT a Postgres/Qdrant simulator. Trust-rank dissent diversion,
//! ANN ranking, cursor pagination and snapshot/restore live in the
//! backends and are asserted against the real stack in
//! `crates/klams-service/tests/`. Faking them here would produce tests
//! that pass while production breaks — the failure mode this sprint
//! exists to remove. Where a behaviour needs the backend, the test for
//! it stays docker-gated and says so.

#![allow(dead_code)]

use async_trait::async_trait;
use klams_mcp::tools::{memory_delete::DeleteCaller, McpState};
use klams_store::{EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use klams_types::{
    AppendEvent, AuthorRecord, Event, Fact, FactWriteOutcome, IndexKnowledge, KnowledgeItem,
    MaintenanceState, RegisterAuthorArgs, Scope, UpsertFact,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use uuid::Uuid;

/// Dimension of the zero vectors `MemStore` hands back. Hermetic tests
/// never inspect the values — the constant exists so the suite carries
/// ONE dim, matching the deployed embedder shape (Qwen3-Embedding-0.6B,
/// 1024, sprint 028) instead of scattering the retired 384.
pub const TEST_EMBED_DIM: usize = 1024;

/// Fresh `McpState` over a fresh `MemStore` — the shared hermetic
/// harness entry point (sprint 034, was copied per-file before).
pub fn state() -> (McpState<MemStore>, Arc<MemStore>) {
    let store = Arc::new(MemStore::new());
    let st = McpState::new(
        Arc::clone(&store),
        Arc::new(MaintenanceState::default()),
        klams_types::ApiConfig::default(),
    );
    (st, store)
}

/// A `memory_delete` caller with an explicit scope set.
pub fn caller(author_id: Uuid, scopes: Vec<Scope>) -> DeleteCaller {
    DeleteCaller { author_id, scopes }
}

/// A caller holding `[read, write]` — may only curate its own rows.
pub fn writer(author_id: Uuid) -> DeleteCaller {
    caller(author_id, vec![Scope::Read, Scope::Write])
}

/// A caller that also holds `manage`, the cross-author curation tier
/// (sprint 025 #633).
pub fn curator(author_id: Uuid) -> DeleteCaller {
    caller(author_id, vec![Scope::Read, Scope::Write, Scope::Manage])
}

/// An all-`None` `register_author` input for `agent_name`.
pub fn author_input(agent_name: &str) -> klams_mcp::tools::register_author::RegisterAuthorInput {
    klams_mcp::tools::register_author::RegisterAuthorInput {
        agent_name: agent_name.to_string(),
        model: None,
        session_title: None,
        repo: None,
        client_app: None,
        client_version: None,
        extra: serde_json::json!({}),
    }
}

#[derive(Default)]
struct Inner {
    authors: Vec<AuthorRecord>,
    facts: Vec<Fact>,
    events: Vec<Event>,
    knowledge: Vec<KnowledgeItem>,
    /// point id → author id, mirroring the Qdrant payload field.
    knowledge_authors: HashMap<Uuid, Uuid>,
    /// Soft-deleted knowledge points: id → who deleted it.
    deleted_knowledge: HashMap<Uuid, Uuid>,
    deleted_facts: Vec<Uuid>,
    fact_authors: HashMap<Uuid, Uuid>,
    /// Bumped by `touch_author_last_seen_at`, so tests can assert the
    /// FR-005 activity touch actually happens.
    touches: Vec<Uuid>,
}

/// In-memory `Store`. Cheap to construct; every method that the MCP
/// tools call is implemented against real state.
#[derive(Default)]
pub struct MemStore {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for MemStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemStore").finish_non_exhaustive()
    }
}

impl MemStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Seed an author directly, skipping the `register_author` tool.
    pub fn seed_author(&self, agent_name: &str) -> Uuid {
        let now = chrono::Utc::now();
        let rec = AuthorRecord {
            id: Uuid::now_v7(),
            agent_name: agent_name.to_string(),
            model: None,
            session_title: None,
            repo: None,
            client_app: Some("mem-store".into()),
            client_version: None,
            extra: serde_json::json!({}),
            created_at: now,
            last_seen_at: now,
        };
        let id = rec.id;
        self.lock().authors.push(rec);
        id
    }

    /// Seed a knowledge point owned by `author`, as the scanner or a
    /// prior agent write would have left it.
    pub fn seed_knowledge(&self, author: Uuid, text: &str) -> Uuid {
        let id = Uuid::now_v7();
        let item = KnowledgeItem {
            id,
            text: text.to_string(),
            source: klams_types::Source::AgentProposal,
            tags: Vec::new(),
            repo: None,
            file: None,
            machine: None,
            content_hash: klams_core::knowledge_write::sha256_hex(text),
            machines: Vec::new(),
            chunk_index: None,
            language: None,
            heading_path: None,
            volatility: None,
            supersedes: None,
            superseded_by: None,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        let mut g = self.lock();
        g.knowledge.push(item);
        g.knowledge_authors.insert(id, author);
        id
    }

    /// How many times `touch_author_last_seen_at` fired for `author`.
    pub fn touch_count(&self, author: Uuid) -> usize {
        self.lock().touches.iter().filter(|t| **t == author).count()
    }

    pub fn fact_count(&self) -> usize {
        self.lock().facts.len()
    }

    pub fn event_count(&self) -> usize {
        self.lock().events.len()
    }

    pub fn knowledge_count(&self) -> usize {
        self.lock().knowledge.len()
    }

    pub fn is_knowledge_soft_deleted(&self, id: Uuid) -> bool {
        self.lock().deleted_knowledge.contains_key(&id)
    }

    pub fn list_all_authors_len(&self) -> usize {
        self.lock().authors.len()
    }

    /// The stored text of a knowledge point, if present.
    pub fn knowledge_text(&self, id: Uuid) -> Option<String> {
        self.lock()
            .knowledge
            .iter()
            .find(|k| k.id == id)
            .map(|k| k.text.clone())
    }
}

fn unsupported(what: &str) -> StoreError {
    StoreError::Other(format!(
        "MemStore does not implement {what} — if a test needs it, either \
         implement it here honestly or make the test docker-gated"
    ))
}

#[async_trait]
impl Store for MemStore {
    async fn upsert_fact_v2(&self, req: UpsertFact) -> StoreResult<FactWriteOutcome> {
        // Trust-rank dissent diversion is a backend concern (see the
        // module doc): every mock write persists.
        let now = OffsetDateTime::now_utc();
        let id = req.explicit_id.unwrap_or_else(Uuid::now_v7);
        let fact = Fact {
            id,
            fact_type: req.fact_type,
            payload: req.payload,
            version: 1,
            source: req.source,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            dissent_count: 0,
            last_used_at: None,
            created_at: now,
            updated_at: now,
        };
        let mut g = self.lock();
        g.fact_authors.insert(id, req.author_id);
        g.facts.retain(|f| f.id != id);
        g.facts.push(fact.clone());
        Ok(FactWriteOutcome::Persisted { fact })
    }

    async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        let ev = Event {
            id: Uuid::now_v7(),
            task_id: req.task_id,
            category: req.category,
            payload: req.payload,
            source: req.source,
            created_at: OffsetDateTime::now_utc(),
        };
        self.lock().events.push(ev.clone());
        Ok(ev)
    }

    async fn index_knowledge(&self, _req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        Err(unsupported(
            "index_knowledge (use index_knowledge_with_embedding)",
        ))
    }

    async fn list_facts(&self, _q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        Ok((self.lock().facts.clone(), None))
    }

    async fn list_events(&self, _q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        Ok((self.lock().events.clone(), None))
    }

    async fn search_knowledge(
        &self,
        _v: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        // Ranking is a backend concern. Return live points in insertion
        // order with a constant score: enough to assert "the handler
        // projects and filters what the store gave it", not enough to
        // pretend anything about relevance.
        let g = self.lock();
        Ok(g.knowledge
            .iter()
            .filter(|k| !g.deleted_knowledge.contains_key(&k.id))
            .take(top_k as usize)
            .map(|k| (k.clone(), 1.0_f32))
            .collect())
    }

    async fn search_text(&self, _q: &str, _k: u32) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        Ok((vec![], vec![]))
    }

    async fn find_knowledge_by_content_hash(&self, hash: &str) -> StoreResult<Option<Uuid>> {
        let g = self.lock();
        Ok(g.knowledge
            .iter()
            .find(|k| k.content_hash == hash && !g.deleted_knowledge.contains_key(&k.id))
            .map(|k| k.id))
    }

    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        Ok(self.lock().knowledge.iter().find(|k| k.id == id).cloned())
    }

    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; TEST_EMBED_DIM])
    }

    async fn embed_document(&self, _text: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; TEST_EMBED_DIM])
    }

    async fn touch_author_last_seen_at(&self, id: Uuid) -> StoreResult<u64> {
        self.lock().touches.push(id);
        Ok(1)
    }

    // ---- authors -----------------------------------------------------

    async fn get_author_by_id(&self, id: Uuid) -> StoreResult<Option<AuthorRecord>> {
        Ok(self.lock().authors.iter().find(|a| a.id == id).cloned())
    }

    async fn get_author_by_agent_name(&self, name: &str) -> StoreResult<Option<AuthorRecord>> {
        Ok(self
            .lock()
            .authors
            .iter()
            .find(|a| a.agent_name == name)
            .cloned())
    }

    async fn insert_author(
        &self,
        args: RegisterAuthorArgs,
        explicit_id: Option<Uuid>,
    ) -> StoreResult<AuthorRecord> {
        let now = chrono::Utc::now();
        let rec = AuthorRecord {
            id: explicit_id.unwrap_or_else(Uuid::now_v7),
            agent_name: args.agent_name,
            model: args.model,
            session_title: args.session_title,
            repo: args.repo,
            client_app: args.client_app,
            client_version: args.client_version,
            extra: args.extra,
            created_at: now,
            last_seen_at: now,
        };
        self.lock().authors.push(rec.clone());
        Ok(rec)
    }

    async fn list_all_authors(&self) -> StoreResult<Vec<AuthorRecord>> {
        Ok(self.lock().authors.clone())
    }

    // ---- facts -------------------------------------------------------

    async fn fact_exists_any(&self, id: Uuid) -> StoreResult<bool> {
        Ok(self.lock().facts.iter().any(|f| f.id == id))
    }

    async fn fact_owner(&self, id: Uuid) -> StoreResult<Option<Uuid>> {
        Ok(self.lock().fact_authors.get(&id).copied())
    }

    async fn soft_delete_fact(&self, id: Uuid, by_author_id: Uuid) -> StoreResult<bool> {
        let mut g = self.lock();
        if !g.facts.iter().any(|f| f.id == id) {
            return Ok(false);
        }
        g.deleted_facts.push(id);
        g.touches.push(by_author_id);
        Ok(true)
    }

    async fn hard_delete_fact(&self, id: Uuid) -> StoreResult<bool> {
        let mut g = self.lock();
        let before = g.facts.len();
        g.facts.retain(|f| f.id != id);
        g.deleted_facts.retain(|d| *d != id);
        Ok(g.facts.len() != before)
    }

    async fn restore_fact(&self, id: Uuid) -> StoreResult<bool> {
        let mut g = self.lock();
        let before = g.deleted_facts.len();
        g.deleted_facts.retain(|d| *d != id);
        Ok(g.deleted_facts.len() != before)
    }

    async fn fetch_facts_with_authors(
        &self,
        ids: &[Uuid],
    ) -> StoreResult<Vec<(Fact, AuthorRecord)>> {
        let g = self.lock();
        let mut out = Vec::new();
        for id in ids {
            let Some(fact) = g.facts.iter().find(|f| f.id == *id) else {
                continue;
            };
            let author_id = g.fact_authors.get(id).copied();
            let Some(author) = author_id.and_then(|a| g.authors.iter().find(|r| r.id == a)) else {
                continue;
            };
            out.push((fact.clone(), author.clone()));
        }
        Ok(out)
    }

    // ---- events ------------------------------------------------------

    async fn event_exists(&self, id: Uuid) -> StoreResult<bool> {
        Ok(self.lock().events.iter().any(|e| e.id == id))
    }

    // ---- knowledge ---------------------------------------------------

    async fn index_knowledge_with_embedding(
        &self,
        req: IndexKnowledge,
        _embedding: Vec<f32>,
    ) -> StoreResult<KnowledgeItem> {
        let item = KnowledgeItem {
            id: req.id,
            text: req.text,
            source: req.source,
            tags: req.tags,
            repo: req.repo,
            file: req.file,
            machine: req.machine,
            content_hash: req.content_hash,
            machines: Vec::new(),
            chunk_index: req.chunk_index,
            language: req.language,
            heading_path: req.heading_path,
            volatility: req.volatility,
            supersedes: req.supersedes,
            superseded_by: None,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        let mut g = self.lock();
        g.knowledge.push(item.clone());
        g.knowledge_authors.insert(item.id, req.author_id);
        Ok(item)
    }

    async fn knowledge_authors_by_ids(&self, ids: &[Uuid]) -> StoreResult<HashMap<Uuid, Uuid>> {
        let g = self.lock();
        Ok(ids
            .iter()
            .filter_map(|id| g.knowledge_authors.get(id).map(|a| (*id, *a)))
            .collect())
    }

    async fn point_exists_any(&self, id: Uuid) -> StoreResult<bool> {
        Ok(self.lock().knowledge.iter().any(|k| k.id == id))
    }

    async fn point_is_soft_deleted(&self, id: Uuid) -> StoreResult<Option<bool>> {
        let g = self.lock();
        if !g.knowledge.iter().any(|k| k.id == id) {
            return Ok(None);
        }
        Ok(Some(g.deleted_knowledge.contains_key(&id)))
    }

    async fn soft_delete_payload(
        &self,
        id: Uuid,
        by_author_id: Uuid,
        _when: OffsetDateTime,
    ) -> StoreResult<()> {
        self.lock().deleted_knowledge.insert(id, by_author_id);
        Ok(())
    }

    async fn restore_payload(&self, id: Uuid) -> StoreResult<()> {
        self.lock().deleted_knowledge.remove(&id);
        Ok(())
    }

    async fn hard_delete_point(&self, id: Uuid) -> StoreResult<()> {
        let mut g = self.lock();
        g.knowledge.retain(|k| k.id != id);
        g.knowledge_authors.remove(&id);
        g.deleted_knowledge.remove(&id);
        Ok(())
    }

    async fn search_knowledge_curated(
        &self,
        v: Vec<f32>,
        top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        self.search_knowledge(v, top_k).await
    }

    async fn get_point_vector(&self, id: Uuid) -> StoreResult<Option<Vec<f32>>> {
        Ok(self
            .lock()
            .knowledge
            .iter()
            .any(|k| k.id == id)
            .then(|| vec![0.0; TEST_EMBED_DIM]))
    }
}
