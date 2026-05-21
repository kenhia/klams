//! Contract tests for `GET /memory/policy` (sprint 003 US5 / T011).
//!
//! Exercises the live axum router with a minimal mock `Store`.
//! Every test here is written to be the TDD red bar for T012/T013 —
//! they MUST fail before the handler + route land.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{MemoryQueue, PolicyTable};
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, UpsertFact};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct NullStore;

#[async_trait]
impl Store for NullStore {
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
            store: Arc::new(NullStore),
            queue,
            queue_capacity: 8,
            workers: 1,
            started_at: std::time::Instant::now(),
            validators: Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: Arc::new(klams_core::context::ContextBuilder::new(klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4), 100)),
        },
        "test-bearer",
    )
}

async fn get_policy(app: axum::Router, with_auth: bool) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(Method::GET).uri("/memory/policy");
    if with_auth {
        builder = builder.header(header::AUTHORIZATION, "Bearer test-bearer");
    }
    let req = builder.body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, v)
}

#[tokio::test]
async fn policy_endpoint_returns_all_four_sources() {
    let (status, v) = get_policy(router(), true).await;
    assert_eq!(status, StatusCode::OK);
    let obj = v.as_object().expect("response must be a JSON object");
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["AgentProposal", "Controller", "Task", "User"]);
    for k in ["User", "Controller", "Task", "AgentProposal"] {
        let entry = &obj[k];
        assert!(entry["rank"].is_u64(), "{k} missing numeric rank");
        assert!(entry["description"].is_string(), "{k} missing description");
    }
}

#[tokio::test]
async fn policy_endpoint_ranks_are_strictly_descending() {
    let (status, v) = get_policy(router(), true).await;
    assert_eq!(status, StatusCode::OK);
    let r = |k: &str| v[k]["rank"].as_u64().unwrap();
    assert!(
        r("User") > r("Controller")
            && r("Controller") > r("Task")
            && r("Task") > r("AgentProposal"),
        "ranks not strictly descending: {v}"
    );
}

#[tokio::test]
async fn policy_endpoint_requires_bearer() {
    let (status, v) = get_policy(router(), false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(v["code"], "unauthorized");
}

#[tokio::test]
async fn policy_endpoint_matches_dispatcher() {
    let (status, v) = get_policy(router(), true).await;
    assert_eq!(status, StatusCode::OK);
    let served: PolicyTable = serde_json::from_value(v).expect("served JSON must round-trip");
    assert_eq!(served, PolicyTable::default());
}
