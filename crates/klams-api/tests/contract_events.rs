//! Contract test for `/memory/events` per `contracts/openapi.yaml`.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, Source, UpsertFact};
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct MockStore {
    events: Mutex<Vec<Event>>,
}

#[async_trait]
impl Store for MockStore {
    async fn upsert_fact(&self, _req: UpsertFact) -> StoreResult<Fact> {
        unimplemented!()
    }
    async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        let e = Event {
            id: req.id,
            task_id: req.task_id,
            category: req.category,
            payload: req.payload,
            source: req.source,
            created_at: OffsetDateTime::now_utc(),
        };
        self.events.lock().unwrap().push(e.clone());
        Ok(e)
    }
    async fn index_knowledge(&self, _r: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        unimplemented!()
    }
    async fn list_facts(&self, _q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        Ok((vec![], None))
    }
    async fn list_events(&self, _q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        Ok((self.events.lock().unwrap().clone(), None))
    }
    async fn search_knowledge(
        &self,
        _v: Vec<f32>,
        _k: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        Ok(vec![])
    }
    async fn search_text(&self, _q: &str, _k: u32) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        Ok((vec![], vec![]))
    }
    async fn find_knowledge_by_content_hash(&self, _h: &str) -> StoreResult<Option<Uuid>> {
        Ok(None)
    }
    async fn get_knowledge(&self, _id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        Ok(None)
    }

    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
}

fn router() -> axum::Router {
    let store = Arc::new(MockStore::default());
    let (queue, rx) = MemoryQueue::new(32);
    let _w = spawn_workers(1, rx, Arc::clone(&store));
    build_router(
        ApiState {
            store,
            queue,
            queue_capacity: 32,
            workers: 1,
            started_at: std::time::Instant::now(),
            validators: std::sync::Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: std::sync::Arc::new(klams_core::context::ContextBuilder::new(
                klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
                100,
            )),
            maintenance: klams_types::MaintenanceState::default(),
        },
        "test-bearer",
    )
}

#[tokio::test]
async fn post_events_returns_202_with_id() {
    let app = router();
    let body = serde_json::json!({
        "task_id": Uuid::now_v7(),
        "category": "started",
        "payload": {"k": "v"},
        "source": "Controller"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/events")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        v.get("id").and_then(|x| x.as_str()).is_some(),
        "must contain id: string"
    );
    assert_eq!(v["path"], "canonical");
}

#[tokio::test]
async fn post_events_missing_auth_is_401() {
    let app = router();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/events")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_events_invalid_payload_is_validation_error() {
    let app = router();
    let body = serde_json::json!({
        "category": "",
        "payload": {"k": "v"},
        "source": "Controller"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/events")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            resp.status(),
            StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "expected 4xx, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn get_events_returns_event_page_shape() {
    let app = router();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/memory/events")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("items").is_some_and(serde_json::Value::is_array));
}

// Source import kept for symmetry with future negative tests.
#[allow(dead_code)]
fn _source_use() -> Source {
    Source::Controller
}

#[tokio::test]
async fn get_events_accepts_service_task_id_and_since() {
    // T025 — `?category=&service=&task_id=&since=` must parse and 200.
    let app = router();
    let uri = "/memory/events?category=Service&service=qdrant&task_id=ansible-00000000000000000000000000000000&since=2024-01-01T00:00:00Z";
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
