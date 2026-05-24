//! Contract tests for `/memory/dissents/*` endpoints (T022).
//!
//! Validates wire-shape conformance against
//! `specs/002-safety-and-write-ops/contracts/openapi.yaml` using an
//! in-memory mock `Store`.

use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use klams_api::{build_router, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{DissentQuery, EventQuery, FactQuery, Store, StoreError, StoreResult, TextHit};
use klams_types::{
    AppendEvent, Dissent, DissentStatus, Event, Fact, FactType, FactWriteOutcome, IndexKnowledge,
    KnowledgeItem, Source, UpsertFact,
};
use std::sync::Arc;
use time::OffsetDateTime;
use tower::ServiceExt;
use uuid::Uuid;

fn nil_uuid() -> Uuid {
    Uuid::nil()
}

fn sample_dissent() -> Dissent {
    Dissent {
        id: nil_uuid(),
        fact_id: nil_uuid(),
        proposed_payload: serde_json::json!({"name": "Bob"}),
        source: Source::AgentProposal,
        status: DissentStatus::Pending,
        submitted_at: OffsetDateTime::now_utc(),
        last_seen_at: OffsetDateTime::now_utc(),
        submission_count: 1,
        resolved_at: None,
        resolved_by_source: None,
    }
}

#[derive(Debug)]
enum Mode {
    NotFound,
    Ok(Dissent),
    Gone,
    VersionConflict(i32),
    Promoted,
}

#[derive(Debug)]
struct MockStore(Mode);

#[async_trait]
impl Store for MockStore {
    async fn upsert_fact(&self, _req: UpsertFact) -> StoreResult<Fact> {
        unimplemented!()
    }
    async fn upsert_fact_v2(&self, _req: UpsertFact) -> StoreResult<FactWriteOutcome> {
        Ok(FactWriteOutcome::Dissented {
            dissent_id: Uuid::now_v7(),
            fact_id: Uuid::now_v7(),
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

    async fn list_dissents(&self, _q: DissentQuery) -> StoreResult<(Vec<Dissent>, Option<String>)> {
        match &self.0 {
            Mode::Ok(d) => Ok((vec![d.clone()], None)),
            _ => Ok((vec![], None)),
        }
    }
    async fn get_dissent(&self, _id: Uuid) -> StoreResult<Option<Dissent>> {
        match &self.0 {
            Mode::Ok(d) => Ok(Some(d.clone())),
            _ => Ok(None),
        }
    }
    async fn promote_dissent(&self, _id: Uuid, _src: Source, _ev: i32) -> StoreResult<Fact> {
        match &self.0 {
            Mode::Gone => Err(StoreError::Gone("already resolved".into())),
            Mode::VersionConflict(cv) => Err(StoreError::VersionConflict {
                current_version: *cv,
            }),
            Mode::Promoted => {
                let now = OffsetDateTime::now_utc();
                Ok(Fact {
                    id: nil_uuid(),
                    fact_type: FactType::UserFact,
                    payload: serde_json::json!({"name": "Bob"}),
                    version: 2,
                    source: Source::User,
                    confidence: 1.0,
                    decay_weight: 1.0,
                    use_count: 0,
                    dissent_count: 0,
                    last_used_at: None,
                    created_at: now,
                    updated_at: now,
                })
            }
            _ => Err(StoreError::Other("unset".into())),
        }
    }
    async fn discard_dissent(&self, _id: Uuid, src: Source) -> StoreResult<Dissent> {
        let mut d = sample_dissent();
        d.status = DissentStatus::Discarded;
        d.resolved_at = Some(OffsetDateTime::now_utc());
        d.resolved_by_source = Some(src);
        Ok(d)
    }
}

fn app_with(mode: Mode) -> axum::Router {
    let store = Arc::new(MockStore(mode));
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

async fn request(
    app: axum::Router,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, "Bearer test-bearer")
        .header(header::CONTENT_TYPE, "application/json");
    let body = match body {
        Some(b) => Body::from(serde_json::to_vec(&b).unwrap()),
        None => Body::empty(),
    };
    let resp = app.oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 16 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

#[tokio::test]
async fn list_dissents_page_shape() {
    let app = app_with(Mode::Ok(sample_dissent()));
    let (status, v) = request(app, Method::GET, "/memory/dissents", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(v["items"].is_array());
    let item = &v["items"][0];
    for k in [
        "id",
        "fact_id",
        "proposed_payload",
        "source",
        "status",
        "submitted_at",
        "last_seen_at",
        "submission_count",
    ] {
        assert!(item.get(k).is_some(), "missing {k}: {item}");
    }
}

#[tokio::test]
async fn get_dissent_404_when_missing() {
    let app = app_with(Mode::NotFound);
    let (status, v) = request(
        app,
        Method::GET,
        &format!("/memory/dissents/{}", Uuid::nil()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v["code"], "not_found");
}

#[tokio::test]
async fn promote_requires_trust() {
    let app = app_with(Mode::Promoted);
    let (status, v) = request(
        app,
        Method::POST,
        &format!("/memory/dissents/{}/promote", Uuid::nil()),
        Some(serde_json::json!({"source": "AgentProposal", "expected_version": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(v["code"], "trust_required");
}

#[tokio::test]
async fn promote_gone_when_resolved() {
    let app = app_with(Mode::Gone);
    let (status, v) = request(
        app,
        Method::POST,
        &format!("/memory/dissents/{}/promote", Uuid::nil()),
        Some(serde_json::json!({"source": "User", "expected_version": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(v["code"], "gone");
}

#[tokio::test]
async fn promote_version_conflict() {
    let app = app_with(Mode::VersionConflict(7));
    let (status, v) = request(
        app,
        Method::POST,
        &format!("/memory/dissents/{}/promote", Uuid::nil()),
        Some(serde_json::json!({"source": "User", "expected_version": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(v["code"], "version_conflict");
    assert_eq!(v["current_version"], 7);
}

#[tokio::test]
async fn promote_ok_returns_updated_fact() {
    let app = app_with(Mode::Promoted);
    let (status, v) = request(
        app,
        Method::POST,
        &format!("/memory/dissents/{}/promote", Uuid::nil()),
        Some(serde_json::json!({"source": "User", "expected_version": 1})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["version"], 2);
    assert_eq!(v["source"], "User");
}

#[tokio::test]
async fn discard_ok_returns_resolved_dissent() {
    let app = app_with(Mode::Ok(sample_dissent()));
    let (status, v) = request(
        app,
        Method::POST,
        &format!("/memory/dissents/{}/discard", Uuid::nil()),
        Some(serde_json::json!({"source": "Controller"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["status"], "discarded");
    assert_eq!(v["resolved_by_source"], "Controller");
}
