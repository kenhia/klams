//! Contract test for `/memory/facts` endpoints.
//!
//! Exercises the live axum router via `tower::ServiceExt::oneshot`
//! with an in-memory mock `Store`, and asserts the HTTP request/
//! response shapes match `specs/001-initial-mvp/contracts/openapi.yaml`.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, UpsertFact};
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct MockStore;

#[async_trait]
impl Store for MockStore {
    async fn upsert_fact(&self, req: UpsertFact) -> StoreResult<Fact> {
        let now = OffsetDateTime::now_utc();
        Ok(Fact {
            id: req.explicit_id.unwrap_or_else(Uuid::now_v7),
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
        })
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

    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
}

fn router() -> axum::Router {
    let store = Arc::new(MockStore);
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
        },
        "test-bearer",
    )
}

#[tokio::test]
async fn post_facts_returns_persisted_fact_shape() {
    let app = router();
    let body = serde_json::json!({
        "type": "UserFact",
        "payload": {"name": "Ada"},
        "source": "Controller"
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/facts")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for k in [
        "id",
        "type",
        "payload",
        "version",
        "source",
        "confidence",
        "decay_weight",
        "use_count",
        "created_at",
        "updated_at",
    ] {
        assert!(
            v.get(k).is_some(),
            "missing required field `{k}` in Fact response"
        );
    }
    assert_eq!(v["type"], "UserFact");
    assert_eq!(v["source"], "Controller");
    assert_eq!(v["version"], 1);
    // Sprint 003 FR-016: every write response carries `path`.
    assert_eq!(v["path"], "canonical");
    assert!(v.get("dissent_id").is_none());
}

#[tokio::test]
async fn post_facts_missing_auth_is_401() {
    let app = router();
    let body = serde_json::json!({"type": "UserFact", "payload": {}, "source": "User"});
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/facts")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["code"], "unauthorized");
    assert!(v.get("message").is_some());
}

#[tokio::test]
async fn get_facts_returns_fact_page_shape() {
    let app = router();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/memory/facts")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("items").is_some_and(serde_json::Value::is_array));
}

#[tokio::test]
async fn malformed_json_is_validation_error() {
    let app = router();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/facts")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"type": "UserFact"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::BAD_REQUEST
            || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "got status {}",
        resp.status()
    );
}

// ---- T013: 422 validation_error wire-shape coverage -------------

async fn post_facts_v(body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let app = router();
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/facts")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

fn assert_validation_detail(v: &serde_json::Value, rule: &str) {
    assert_eq!(v["code"], "validation_error", "wire body: {v}");
    let details = v["details"].as_array().expect("details array");
    assert!(
        details.iter().any(|d| d["rule"].as_str() == Some(rule)
            && d.get("field").is_some()
            && d.get("message").is_some()),
        "expected a detail with rule=`{rule}`; got {v}"
    );
}

#[tokio::test]
async fn validation_missing_required_field() {
    let (status, v) = post_facts_v(serde_json::json!({
        "type": "UserFact",
        "payload": {},
        "source": "Controller"
    }))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_validation_detail(&v, "required");
}

#[tokio::test]
async fn validation_hostname_shape_rejected_for_user_source() {
    // FR-006: sanity rules apply to every source.
    let (status, v) = post_facts_v(serde_json::json!({
        "type": "UserFact",
        "payload": {"name": "Ada", "hostname": "BAD_HOST!!"},
        "source": "User"
    }))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_validation_detail(&v, "hostname_shape");
}

#[tokio::test]
async fn validation_far_future_timestamp_rejected() {
    let (status, v) = post_facts_v(serde_json::json!({
        "type": "UserFact",
        "payload": {"name": "Ada", "noticed_at": "3030-01-01T00:00:00Z"},
        "source": "Controller"
    }))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_validation_detail(&v, "timestamp_range");
}

#[tokio::test]
async fn validation_task_status_enum_rejected() {
    let (status, v) = post_facts_v(serde_json::json!({
        "type": "TaskFact",
        "payload": {"task_id": "550e8400-e29b-41d4-a716-446655440000", "status": "nope"},
        "source": "Controller"
    }))
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_validation_detail(&v, "enum");
}
