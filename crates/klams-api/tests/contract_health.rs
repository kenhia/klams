//! Contract test for `GET /healthz`.
//!
//! Uses an in-memory mock `Store` whose three health probes return
//! `Ok(())` (via trait defaults), so the snapshot must be all-Ok and
//! the response code 200. The test asserts the JSON shape matches
//! the `OpenAPI` `HealthSnapshot` schema.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::MemoryQueue;
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, UpsertFact};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct HealthyStore;

#[async_trait]
impl Store for HealthyStore {
    async fn upsert_fact(&self, _req: UpsertFact) -> StoreResult<Fact> {
        unimplemented!()
    }
    async fn append_event(&self, _req: AppendEvent) -> StoreResult<Event> {
        unimplemented!()
    }
    async fn index_knowledge(&self, _req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        unimplemented!()
    }
    async fn list_facts(&self, _q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        Ok((vec![], None))
    }
    async fn list_events(&self, _q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        Ok((vec![], None))
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
    async fn embed_query(&self, _q: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
}

fn router() -> axum::Router {
    let (queue, _rx) = MemoryQueue::new(8);
    build_router(
        ApiState {
            store: Arc::new(HealthyStore),
            queue,
            queue_capacity: 8,
            workers: 2,
            started_at: std::time::Instant::now(),
            validators: std::sync::Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: std::sync::Arc::new(klams_core::context::ContextBuilder::new(
                klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
                100,
            )),
        },
        "test-token",
    )
}

#[tokio::test]
async fn healthz_returns_200_with_full_snapshot() {
    let app = router();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(v["status"], "Ok");
    for key in ["postgres", "qdrant", "embeddings"] {
        assert_eq!(v[key]["state"], "Ok", "subsystem {key}");
    }
    assert_eq!(v["queue"]["capacity"], 8);
    assert_eq!(v["queue"]["workers"], 2);
    assert!(v["queue"]["depth"].is_number());
    assert!(v["version"].is_string());
    assert!(v["uptime_seconds"].is_number());
}

#[tokio::test]
async fn healthz_is_unauthenticated() {
    let app = router();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn healthz_with_contract_v1_includes_contract_field() {
    let app = router();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz?contract=v1")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["contract"], "v1");
}

#[tokio::test]
async fn healthz_without_contract_query_unchanged() {
    let app = router();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v.get("contract").is_none());
}
