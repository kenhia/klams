//! T020: queue tests — bounded capacity, `QueueFull` on overflow,
//! worker drains an `AppendEvent` job.

use async_trait::async_trait;
use klams_core::{spawn_workers, MemoryQueue, WriteJob};
use klams_store::{EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, Source, UpsertFact};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Default)]
struct RecordingStore {
    events: Mutex<Vec<Event>>,
}

#[async_trait]
impl Store for RecordingStore {
    async fn upsert_fact(&self, _req: UpsertFact) -> StoreResult<Fact> {
        Err(StoreError::Other("not used".into()))
    }

    async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        let evt = Event {
            id: req.id,
            task_id: req.task_id,
            category: req.category,
            payload: req.payload,
            source: req.source,
            created_at: time::OffsetDateTime::now_utc(),
        };
        self.events.lock().await.push(evt.clone());
        Ok(evt)
    }

    async fn index_knowledge(&self, _req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        Err(StoreError::Other("not used".into()))
    }

    async fn list_facts(&self, _q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        Ok((vec![], None))
    }

    async fn list_events(&self, _q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        Ok((self.events.lock().await.clone(), None))
    }

    async fn search_knowledge(
        &self,
        _query_vector: Vec<f32>,
        _top_k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        Ok(vec![])
    }

    async fn search_text(
        &self,
        _query: &str,
        _top_k: u32,
    ) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        Ok((vec![], vec![]))
    }

    async fn find_knowledge_by_content_hash(
        &self,
        _hash: &str,
        _source_file: Option<&str>,
        _machine: Option<&str>,
    ) -> StoreResult<Option<Uuid>> {
        Ok(None)
    }

    async fn get_knowledge(&self, _id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        Ok(None)
    }

    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
}

fn sample_event() -> AppendEvent {
    AppendEvent {
        id: Uuid::now_v7(),
        task_id: None,
        category: "Execution".into(),
        payload: serde_json::json!({"step": "init"}),
        source: Source::Controller,
        author_id: klams_types::SYSTEM_AUTHOR_ID,
    }
}

#[tokio::test]
async fn queue_accepts_up_to_capacity_then_rejects() {
    let (queue, _rx) = MemoryQueue::new(2);
    assert_eq!(queue.capacity(), 2);

    assert!(queue
        .try_enqueue(WriteJob::append_event(sample_event()))
        .is_ok());
    assert!(queue
        .try_enqueue(WriteJob::append_event(sample_event()))
        .is_ok());
    let third = queue.try_enqueue(WriteJob::append_event(sample_event()));
    assert!(third.is_err(), "third enqueue must overflow");
}

#[tokio::test]
async fn worker_drains_append_event_job() {
    let (queue, rx) = MemoryQueue::new(4);
    let store: Arc<RecordingStore> = Arc::new(RecordingStore::default());
    let _handles = spawn_workers(1, rx, Arc::clone(&store));

    queue
        .try_enqueue(WriteJob::append_event(sample_event()))
        .expect("enqueue should succeed");

    // Wait briefly for the worker to drain the job.
    for _ in 0..50 {
        if !store.events.lock().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let recorded = store.events.lock().await;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].category, "Execution");
}
