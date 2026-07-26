//! Contract tests for `/memory/knowledge/index` and `/memory/knowledge/{id}`.
//!
//! Backed by an in-memory mock `Store` so we exercise the live axum
//! router + worker pipeline without touching Qdrant/Postgres. The
//! mock implements just enough behaviour to round-trip indexed items
//! through a `WriteJob::IndexKnowledge` and serve them back via
//! `get_knowledge`. Content-hash dedupe is exercised end-to-end.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{AppendEvent, Event, Fact, IndexKnowledge, KnowledgeItem, UpsertFact};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct MockStore {
    by_hash: Mutex<HashMap<String, Uuid>>,
    by_id: Mutex<HashMap<Uuid, KnowledgeItem>>,
}

#[async_trait]
impl Store for MockStore {
    async fn upsert_fact(&self, _req: UpsertFact) -> StoreResult<Fact> {
        unimplemented!()
    }
    async fn append_event(&self, _req: AppendEvent) -> StoreResult<Event> {
        unimplemented!()
    }
    async fn index_knowledge(&self, req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        let now = OffsetDateTime::now_utc();
        let item = KnowledgeItem {
            id: req.id,
            text: req.text,
            content_hash: req.content_hash.clone(),
            source: req.source,
            tags: req.tags,
            repo: req.repo,
            file: req.file,
            machine: req.machine,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: now,
            updated_at: now,
        };
        self.by_hash
            .lock()
            .unwrap()
            .insert(req.content_hash, req.id);
        self.by_id.lock().unwrap().insert(req.id, item.clone());
        Ok(item)
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
    async fn find_knowledge_by_content_hash(
        &self,
        h: &str,
        _source_file: Option<&str>,
        _machine: Option<&str>,
    ) -> StoreResult<Option<Uuid>> {
        Ok(self.by_hash.lock().unwrap().get(h).copied())
    }
    async fn get_knowledge(&self, id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        Ok(self.by_id.lock().unwrap().get(&id).cloned())
    }
    async fn delete_knowledge_by_source_file(
        &self,
        sf: &str,
        _machine: Option<&str>,
    ) -> StoreResult<u64> {
        let mut by_id = self.by_id.lock().unwrap();
        let ids: Vec<Uuid> = by_id
            .iter()
            .filter(|(_, it)| it.file.as_deref() == Some(sf))
            .map(|(id, _)| *id)
            .collect();
        for id in &ids {
            by_id.remove(id);
        }
        Ok(ids.len() as u64)
    }
    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        Ok(vec![0.0; 384])
    }
}

fn router_with_store(store: Arc<MockStore>) -> axum::Router {
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
        },
        "test-bearer",
    )
}

async fn post(
    app: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
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

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let resp = app
        .clone()
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
async fn post_index_returns_accepted_with_id() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let (status, body) = post(
        &app,
        "/memory/knowledge/index",
        serde_json::json!({
            "text": "hello world from contract test",
            "source": "Controller",
            "tags": ["t1"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert!(body.get("knowledge_id").is_some());
    assert_eq!(
        body.get("deduped").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(body["path"], "canonical");
}

#[tokio::test]
async fn duplicate_text_returns_deduped_true_with_same_id() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(Arc::clone(&store));

    let req = serde_json::json!({
        "text": "exact duplicate text",
        "source": "Controller"
    });
    let (status1, body1) = post(&app, "/memory/knowledge/index", req.clone()).await;
    assert_eq!(status1, StatusCode::ACCEPTED);
    assert_eq!(body1["deduped"], serde_json::Value::Bool(false));
    let id1 = body1["knowledge_id"].as_str().unwrap().to_string();

    // Wait for worker to persist so dedupe lookup hits.
    for _ in 0..50 {
        if !store.by_hash.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let (status2, body2) = post(&app, "/memory/knowledge/index", req).await;
    assert_eq!(status2, StatusCode::ACCEPTED);
    assert_eq!(body2["deduped"], serde_json::Value::Bool(true));
    assert_eq!(body2["knowledge_id"].as_str().unwrap(), id1);
}

#[tokio::test]
async fn oversized_text_returns_413() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let big = "a".repeat(8193);
    let (status, body) = post(
        &app,
        "/memory/knowledge/index",
        serde_json::json!({"text": big, "source": "Controller"}),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["code"], "payload_too_large");
}

#[tokio::test]
async fn empty_text_returns_400_validation() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let (status, body) = post(
        &app,
        "/memory/knowledge/index",
        serde_json::json!({"text": "   ", "source": "Controller"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "validation_error");
}

#[tokio::test]
async fn get_knowledge_returns_item_when_present() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(Arc::clone(&store));
    let (_s, body) = post(
        &app,
        "/memory/knowledge/index",
        serde_json::json!({"text": "fetch me back", "source": "Controller"}),
    )
    .await;
    let id = body["knowledge_id"].as_str().unwrap().to_string();
    for _ in 0..50 {
        if store
            .by_id
            .lock()
            .unwrap()
            .contains_key(&Uuid::parse_str(&id).unwrap())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let (status, body) = get(&app, &format!("/memory/knowledge/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    for k in [
        "id",
        "text",
        "content_hash",
        "source",
        "tags",
        "created_at",
        "updated_at",
    ] {
        assert!(body.get(k).is_some(), "missing field {k}");
    }
}

#[tokio::test]
async fn get_knowledge_returns_404_for_unknown_id() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let (status, body) = get(&app, &format!("/memory/knowledge/{}", Uuid::now_v7())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["code"], "not_found");
}

#[tokio::test]
async fn knowledge_delete_requires_bearer() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/delete?source_file=%2Ftmp%2Fx")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn knowledge_delete_removes_matching_chunks() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(Arc::clone(&store));
    // Seed two items with the same `file` value directly in the mock.
    let id1 = Uuid::now_v7();
    let id2 = Uuid::now_v7();
    let now = OffsetDateTime::now_utc();
    let mk = |id: Uuid| KnowledgeItem {
        id,
        text: "x".into(),
        content_hash: id.to_string(),
        source: klams_types::Source::Controller,
        tags: vec![],
        repo: None,
        file: Some("/abs/path/note.md".into()),
        machine: None,
        confidence: 1.0,
        decay_weight: 1.0,
        use_count: 0,
        last_used_at: None,
        created_at: now,
        updated_at: now,
    };
    {
        let mut by_id = store.by_id.lock().unwrap();
        by_id.insert(id1, mk(id1));
        by_id.insert(id2, mk(id2));
    }
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/delete?source_file=%2Fabs%2Fpath%2Fnote.md&machine=kubs0")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["deleted"], 2);
    assert_eq!(v["path"], "canonical");
    assert!(store.by_id.lock().unwrap().is_empty());
}

#[tokio::test]
async fn knowledge_delete_missing_source_file_returns_zero() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/delete?source_file=%2Fno%2Fsuch%2Fpath&machine=kubs0")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["deleted"], 0);
    assert_eq!(v["path"], "canonical");
}

/// Sprint 025 (#637) — `machine` is required. Omitting it used to mean
/// "delete this `source_file`'s chunks on every host", so a hand-run
/// cleanup for one machine silently wiped the others.
#[tokio::test]
async fn knowledge_delete_without_machine_is_rejected() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/delete?source_file=%2Fabs%2Fpath%2Fnote.md")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["field"], "machine", "error must name the missing field");
}

/// Blank is the same as absent — a `machine=` with nothing after it
/// must not fall through to the cross-host delete.
#[tokio::test]
async fn knowledge_delete_with_blank_machine_is_rejected() {
    let store = Arc::new(MockStore::default());
    let app = router_with_store(store);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/delete?source_file=%2Fa%2Fb.md&machine=%20%20")
                .header(header::AUTHORIZATION, "Bearer test-bearer")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
