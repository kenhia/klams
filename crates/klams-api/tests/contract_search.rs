//! Contract test for `POST /memory/search`.
//!
//! Verifies the unified search endpoint accepts the request shape
//! described in `specs/001-initial-mvp/contracts/openapi.yaml`,
//! emits a `SearchResults` envelope with `query`/`total`/`results`/
//! `degraded` fields, normalises mixed-type hits into a single
//! interleaved list, and honours the `types` filter.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, Source, UpsertFact};
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct MockStore {
    fail_knowledge: bool,
}

#[async_trait]
impl Store for MockStore {
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
        let now = OffsetDateTime::now_utc();
        Ok(vec![(
            KnowledgeItem {
                id: Uuid::now_v7(),
                text: "knowledge body".into(),
                content_hash: "h".into(),
                source: Source::Controller,
                tags: vec![],
                repo: None,
                file: None,
                machine: None,
                confidence: 1.0,
                decay_weight: 1.0,
                use_count: 0,
                last_used_at: None,
                created_at: now,
                updated_at: now,
            },
            0.85,
        )])
    }
    async fn search_text(&self, _q: &str, _k: u32) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        let f = TextHit {
            id: Uuid::now_v7(),
            score: 0.6,
            payload: serde_json::json!({"summary": "fact-row"}),
        };
        let e = TextHit {
            id: Uuid::now_v7(),
            score: 0.4,
            payload: serde_json::json!({"summary": "event-row"}),
        };
        Ok((vec![f], vec![e]))
    }
    async fn find_knowledge_by_content_hash(&self, _h: &str) -> StoreResult<Option<Uuid>> {
        Ok(None)
    }
    async fn get_knowledge(&self, _id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        Ok(None)
    }
    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        if self.fail_knowledge {
            Err(klams_store::StoreError::Backend("embed failed".into()))
        } else {
            Ok(vec![0.0; 384])
        }
    }
}

fn router_with(store: Arc<MockStore>) -> axum::Router {
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
        },
        "test-bearer",
    )
}

async fn search(app: &axum::Router, body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/search")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, v)
}

#[tokio::test]
async fn search_returns_envelope_with_all_three_types() {
    let app = router_with(Arc::new(MockStore::default()));
    let (status, body) = search(&app, serde_json::json!({"query": "hello", "top_k": 9})).await;
    assert_eq!(status, StatusCode::OK);
    for k in ["query", "total", "results", "degraded"] {
        assert!(body.get(k).is_some(), "missing {k}");
    }
    assert_eq!(body["degraded"], serde_json::Value::Bool(false));
    let results = body["results"].as_array().unwrap();
    assert_eq!(
        results.len(),
        3,
        "should interleave 1 fact + 1 event + 1 knowledge"
    );
    let kinds: Vec<&str> = results
        .iter()
        .map(|h| h["type"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"fact"));
    assert!(kinds.contains(&"event"));
    assert!(kinds.contains(&"knowledge"));
    for hit in results {
        for k in ["type", "id", "score", "preview", "payload"] {
            assert!(hit.get(k).is_some(), "hit missing {k}");
        }
        let score = hit["score"].as_f64().unwrap();
        assert!((0.0..=1.0).contains(&score), "score out of range: {score}");
    }
}

#[tokio::test]
async fn search_types_filter_restricts_results() {
    let app = router_with(Arc::new(MockStore::default()));
    let (status, body) = search(
        &app,
        serde_json::json!({"query": "x", "types": ["knowledge"], "top_k": 5}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().unwrap();
    assert!(!results.is_empty());
    for hit in results {
        assert_eq!(hit["type"], "knowledge");
    }
}

#[tokio::test]
async fn search_sets_degraded_when_knowledge_fails() {
    let app = router_with(Arc::new(MockStore {
        fail_knowledge: true,
    }));
    let (status, body) = search(&app, serde_json::json!({"query": "q"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["degraded"], serde_json::Value::Bool(true));
    let results = body["results"].as_array().unwrap();
    for hit in results {
        assert_ne!(hit["type"], "knowledge");
    }
}

#[tokio::test]
async fn empty_query_returns_400() {
    let app = router_with(Arc::new(MockStore::default()));
    let (status, body) = search(&app, serde_json::json!({"query": "   "})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "validation_error");
}
