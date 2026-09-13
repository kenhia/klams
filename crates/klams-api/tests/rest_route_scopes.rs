//! Sprint 025 (#637) — every protected REST route enforces its scope.
//!
//! Before this sprint `require_scope` was layered on exactly one route
//! (`/v1/memories`). Every other protected route accepted *any* valid
//! credential, so the read-only caller could index knowledge,
//! bulk-delete it, and resolve dissents — the `scopes` list was
//! decorative on this surface.
//!
//! Sprint 052: the callers are `[[auth.identities]]` rows declaring
//! `X-Homelab-Agent`. The gate they exercise is unchanged.
//!
//! These tests assert the gate, not the handlers: a read-only caller
//! must be refused *before* any handler runs, which is why they can use
//! an `unimplemented!()` mock store. If a route regressed to
//! no-scope-check, the mock would panic instead of returning 403 —
//! which is itself a failure, so the tests are honest either way.

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use klams_api::auth::{AuthState, Identity};
use klams_api::{build_router_with_auth, ApiState};
use klams_core::MemoryQueue;
use klams_store::{EventQuery, FactQuery, Store, StoreResult, TextHit};
use klams_types::{
    AppendEvent, Event, Fact, FactWriteOutcome, IndexKnowledge, KnowledgeItem, Scope, UpsertFact,
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const READ_AGENT: &str = "klams-view";
const WRITE_AGENT: &str = "klams-scanner";
const MANAGE_AGENT: &str = "claude";

fn identity(name: &str, scopes: Vec<Scope>) -> Identity {
    Identity {
        agent_name: Arc::new(name.to_string()),
        scopes: Arc::new(scopes),
        label: Some(name.to_string()),
        author_id: klams_types::SYSTEM_AUTHOR_ID,
        nodes: Arc::new(Vec::new()),
    }
}

#[derive(Debug, Default)]
struct PanicStore;

#[async_trait]
impl Store for PanicStore {
    async fn upsert_fact_v2(&self, _req: UpsertFact) -> StoreResult<FactWriteOutcome> {
        unimplemented!("handler must not be reached")
    }
    async fn append_event(&self, _req: AppendEvent) -> StoreResult<Event> {
        unimplemented!("handler must not be reached")
    }
    async fn index_knowledge(&self, _req: IndexKnowledge) -> StoreResult<KnowledgeItem> {
        unimplemented!("handler must not be reached")
    }
    async fn get_knowledge(&self, _id: Uuid) -> StoreResult<Option<KnowledgeItem>> {
        unimplemented!("handler must not be reached")
    }
    async fn find_knowledge_by_content_hash(&self, _hash: &str) -> StoreResult<Option<Uuid>> {
        unimplemented!("handler must not be reached")
    }
    async fn list_facts(&self, _q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        unimplemented!("handler must not be reached")
    }
    async fn list_events(&self, _q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        unimplemented!("handler must not be reached")
    }
    async fn search_text(
        &self,
        _q: &str,
        _limit: u32,
    ) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        unimplemented!("handler must not be reached")
    }
    async fn search_knowledge(
        &self,
        _vec: Vec<f32>,
        _limit: u32,
    ) -> StoreResult<Vec<(KnowledgeItem, f32)>> {
        unimplemented!("handler must not be reached")
    }
    /// The one method that returns rather than panics: the scanner
    /// delete-before-reindex test asserts a write token *does* get
    /// through the gate, so its handler has to be able to finish.
    async fn delete_knowledge_by_source_file(
        &self,
        _sf: &str,
        _machine: Option<&str>,
    ) -> StoreResult<u64> {
        Ok(0)
    }
    async fn embed_query(&self, _query: &str) -> StoreResult<Vec<f32>> {
        unimplemented!("handler must not be reached")
    }
}

fn app() -> axum::Router {
    let (queue, _rx) = MemoryQueue::new(32);
    let auth = AuthState::with_identities(vec![
        // A genuinely read-only consumer — a dashboard (klams-view) or
        // a scrape job. Curation identities are the contrast: resolving
        // a dissent needs `manage` (see docs/auth.md).
        identity(READ_AGENT, vec![Scope::Read]),
        identity(WRITE_AGENT, vec![Scope::Read, Scope::Write]),
        identity(MANAGE_AGENT, vec![Scope::Read, Scope::Write, Scope::Manage]),
    ]);
    build_router_with_auth(
        ApiState {
            store: Arc::new(PanicStore),
            api: klams_types::ApiConfig::default(),
            queue,
            queue_capacity: 32,
            workers: 1,
            started_at: std::time::Instant::now(),
            validators: Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: Arc::new(klams_core::context::ContextBuilder::new(
                klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
                100,
            )),
            maintenance: klams_types::MaintenanceState::default(),
            embed_limit: klams_types::EmbedLimit::default(),
            fusion: klams_types::FusionStrategy::default_rrf(),
            reranker: None,
            rerank_window: 50,
        },
        auth,
    )
}

async fn status(method: Method, uri: &str, agent: &str) -> StatusCode {
    app()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("x-homelab-agent", agent)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// Every route that mutates state. The read-only token must be refused
/// on all of them — this is the sprint's #637 acceptance criterion.
fn mutating_routes() -> Vec<(Method, &'static str)> {
    vec![
        (Method::POST, "/memory/facts"),
        (Method::POST, "/memory/events"),
        (Method::POST, "/memory/knowledge/index"),
        (
            Method::POST,
            "/memory/knowledge/delete?source_file=%2Fa%2Fb.md&machine=kubs0",
        ),
        (
            Method::POST,
            "/memory/dissents/00000000-0000-0000-0000-000000000001/promote",
        ),
        (
            Method::POST,
            "/memory/dissents/00000000-0000-0000-0000-000000000001/discard",
        ),
    ]
}

#[tokio::test]
async fn read_only_token_is_refused_on_every_mutating_route() {
    for (method, uri) in mutating_routes() {
        let got = status(method.clone(), uri, READ_AGENT).await;
        assert_eq!(
            got,
            StatusCode::FORBIDDEN,
            "read-only token must not reach {method} {uri}"
        );
    }
}

/// The refusal is the typed `scope_insufficient` error, naming the tier
/// the caller lacks — not a bare 403.
#[tokio::test]
async fn refusal_names_the_missing_scope() {
    let resp = app()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/index")
                .header("x-homelab-agent", READ_AGENT)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["code"], "scope_insufficient");
    assert!(
        v["message"].as_str().unwrap().contains("write"),
        "message must name the needed scope: {v}"
    );
}

/// Dissent resolution is cross-author curation: promote overwrites a
/// live canonical fact. A plain write token is not enough.
#[tokio::test]
async fn write_token_cannot_resolve_dissents_but_manage_can() {
    let promote = "/memory/dissents/00000000-0000-0000-0000-000000000001/promote";
    assert_eq!(
        status(Method::POST, promote, WRITE_AGENT).await,
        StatusCode::FORBIDDEN,
        "write alone must not resolve dissents"
    );
    assert_ne!(
        status(Method::POST, promote, MANAGE_AGENT).await,
        StatusCode::FORBIDDEN,
        "manage must pass the scope gate"
    );
}

/// The scanner's delete-before-reindex stays Write-tier — gating it at
/// `manage` would break vanished-file cleanup (FR-008).
#[tokio::test]
async fn write_token_may_still_run_delete_before_reindex() {
    let uri = "/memory/knowledge/delete?source_file=%2Fa%2Fb.md&machine=kubs0";
    assert_ne!(
        status(Method::POST, uri, WRITE_AGENT).await,
        StatusCode::FORBIDDEN,
        "scanner cleanup must remain available to a write token"
    );
}

/// Read routes stay reachable by the read-only token — the gate must
/// not have been applied indiscriminately.
#[tokio::test]
async fn read_only_token_still_passes_the_gate_on_read_routes() {
    for uri in [
        "/memory/policy",
        "/memory/dissents",
        "/v1/authors",
        "/v1/memories",
    ] {
        assert_ne!(
            status(Method::GET, uri, READ_AGENT).await,
            StatusCode::FORBIDDEN,
            "read token must pass the scope gate on {uri}"
        );
    }
}

/// A method mismatch must still be 405, not masked into a 403 by the
/// gate — the reason these use `route_layer` rather than `layer`.
#[tokio::test]
async fn method_mismatch_is_405_not_403() {
    assert_eq!(
        status(Method::DELETE, "/memory/facts", READ_AGENT).await,
        StatusCode::METHOD_NOT_ALLOWED
    );
}

/// An unauthenticated request is still 401 — the scope layer sits
/// inside `require_bearer`, not in front of it.
#[tokio::test]
async fn missing_bearer_is_still_401() {
    let got = app()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/memory/knowledge/index")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
        .status();
    assert_eq!(got, StatusCode::UNAUTHORIZED);
}
