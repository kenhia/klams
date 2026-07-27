//! Contract test for `POST /memory/context`.
//!
//! Sprint 005 (Phase 4) — T013. Verifies the `ContextRequest`
//! envelope, the `ContextBundle` response shape, the `query_required`
//! error, unknown-filter-key rejection, and the per-section
//! degradation behaviour (unhealthy store → 200 with
//! `status: unavailable`, not a 5xx).

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, Source, UpsertFact};
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default, Clone, Copy)]
struct MockOpts {
    fail_vector: bool,
    fail_text: bool,
}

#[derive(Debug)]
struct MockStore {
    opts: MockOpts,
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
                text: "kubs0 ships a 24 GiB GPU and CUDA 12.5".into(),
                content_hash: "h".into(),
                source: Source::Controller,
                tags: vec!["gpu".into()],
                repo: Some("ansible-k".into()),
                file: Some("roles/gpu.yml".into()),
                machine: Some("kubs0".into()),
                machines: vec![],
                heading_path: None,
                language: None,
                chunk_index: None,
                volatility: None,
                supersedes: None,
                superseded_by: None,
                confidence: 1.0,
                decay_weight: 1.0,
                use_count: 0,
                last_used_at: None,
                created_at: now,
                updated_at: now,
            },
            0.95,
        )])
    }
    async fn search_text(&self, _q: &str, _k: u32) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        if self.opts.fail_text {
            return Err(StoreError::Backend("text down".into()));
        }
        let f = TextHit {
            id: Uuid::now_v7(),
            score: 0.6,
            payload: serde_json::json!({"summary": "fact-row", "host": "kubs0"}),
        };
        let e = TextHit {
            id: Uuid::now_v7(),
            score: 0.4,
            payload: serde_json::json!({"summary": "event-row", "host": "kubs0"}),
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
        if self.opts.fail_vector {
            Err(StoreError::Embedding("embed down".into()))
        } else {
            Ok(vec![0.0; 384])
        }
    }
}

fn router_with(opts: MockOpts) -> axum::Router {
    let store = Arc::new(MockStore { opts });
    let (queue, rx) = MemoryQueue::new(32);
    let _w = spawn_workers(1, rx, Arc::clone(&store));
    build_router(
        ApiState {
            store,
            api: klams_types::ApiConfig::default(),
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
            embed_limit: klams_types::EmbedLimit::default(),
        },
        "test-bearer",
    )
}

async fn post_context(
    app: &axum::Router,
    body: serde_json::Value,
) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/context")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, headers, json)
}

#[tokio::test]
async fn happy_path_returns_bundle_shape() {
    let app = router_with(MockOpts::default());
    let (status, _h, body) = post_context(
        &app,
        serde_json::json!({
            "query": "kubs0 GPU and CUDA toolkit",
            "token_budget": 4000,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    for field in [
        "facts",
        "knowledge",
        "events",
        "total_spent",
        "truncated",
        "token_encoder",
        "sections",
    ] {
        assert!(body.get(field).is_some(), "missing field {field} in {body}");
    }
    assert!(body["token_encoder"].as_str().is_some());
    assert!(body["total_spent"].as_u64().unwrap() <= 4000);
}

#[tokio::test]
async fn empty_query_returns_400_with_query_required() {
    let app = router_with(MockOpts::default());
    let (status, _h, body) = post_context(
        &app,
        serde_json::json!({ "query": "", "token_budget": 4000 }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"].as_str(), Some("validation_error"));
    assert!(body["message"].as_str().unwrap().contains("query_required"));
}

#[tokio::test]
async fn unknown_filter_key_returns_4xx() {
    let app = router_with(MockOpts::default());
    let (status, _h, _body) = post_context(
        &app,
        serde_json::json!({
            "query": "x",
            "token_budget": 100,
            "filters": { "bogus_key": "value" }
        }),
    )
    .await;
    // axum maps `deny_unknown_fields` violations to 422
    // (Unprocessable Entity); pure-JSON-syntax errors map to 400.
    // Both are acceptable per the OpenAPI contract.
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422, got {status}"
    );
}

#[tokio::test]
async fn zero_budget_returns_truncated_empty_bundle() {
    let app = router_with(MockOpts::default());
    let (status, _h, body) = post_context(
        &app,
        serde_json::json!({ "query": "anything", "token_budget": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["truncated"], serde_json::Value::Bool(true));
    assert_eq!(body["total_spent"], serde_json::Value::from(0u32));
}

#[tokio::test]
async fn vector_down_yields_200_with_unavailable_section() {
    // Text source still healthy; vector down → 200, knowledge
    // section status=unavailable, facts/events still present.
    let app = router_with(MockOpts {
        fail_vector: true,
        fail_text: false,
    });
    let (status, _h, body) = post_context(
        &app,
        serde_json::json!({ "query": "x", "token_budget": 4000 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let knowledge_status = body["sections"]["knowledge"]["status"].as_str();
    assert_eq!(knowledge_status, Some("unavailable"), "body: {body}");
}

#[tokio::test]
async fn all_sources_down_returns_503_with_retry_after() {
    let app = router_with(MockOpts {
        fail_vector: true,
        fail_text: true,
    });
    let (status, headers, _body) = post_context(
        &app,
        serde_json::json!({ "query": "x", "token_budget": 4000 }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(headers.contains_key(header::RETRY_AFTER));
}
