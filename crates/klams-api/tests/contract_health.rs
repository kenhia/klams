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
use klams_types::{
    AppendEvent, Event, Fact, FactWriteOutcome, IndexKnowledge, KnowledgeItem, UpsertFact,
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Debug, Default)]
struct HealthyStore;

#[async_trait]
impl Store for HealthyStore {
    async fn upsert_fact_v2(&self, _req: UpsertFact) -> StoreResult<FactWriteOutcome> {
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
    router_with_reranker(None)
}

fn router_with_reranker(reranker: Option<Arc<klams_store::TeiReranker>>) -> axum::Router {
    let (queue, _rx) = MemoryQueue::new(8);
    build_router(
        ApiState {
            store: Arc::new(HealthyStore),
            queue,
            api: klams_types::ApiConfig::default(),
            queue_capacity: 8,
            workers: 2,
            started_at: std::time::Instant::now(),
            validators: std::sync::Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: std::sync::Arc::new(klams_core::context::ContextBuilder::new(
                klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
                100,
            )),
            maintenance: klams_types::MaintenanceState::default(),
            embed_limit: klams_types::EmbedLimit::default(),
            fusion: klams_types::FusionStrategy::default_rrf(),
            reranker,
            rerank_window: 50,
        },
        "test-token",
    )
}

async fn healthz_json(app: axum::Router) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
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
    // Sprint 036 (#731): no reranker configured → the field is omitted
    // entirely, not present as a permanently-down subsystem.
    assert!(
        v.get("reranker").is_none(),
        "unconfigured reranker must not appear in the snapshot"
    );
}

// ---- Sprint 036 (#731): reranker visibility. The stage is best-effort
// (searches serve the un-reranked order when it is sick), so /healthz
// must SHOW its state without ever letting it flip overall status or
// the HTTP code — "rerank silently off for a week" was the failure this
// exists to prevent.

#[tokio::test]
async fn healthz_reports_a_healthy_reranker_without_affecting_status() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let reranker = Arc::new(klams_store::TeiReranker::new(server.uri()).unwrap());
    let (status, v) = healthz_json(router_with_reranker(Some(reranker))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["status"], "Ok");
    assert_eq!(v["reranker"]["state"], "Ok");
}

#[tokio::test]
async fn a_sick_reranker_is_visible_but_never_fatal() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let reranker = Arc::new(klams_store::TeiReranker::new(server.uri()).unwrap());
    let (status, v) = healthz_json(router_with_reranker(Some(reranker))).await;
    // Visible: the subsystem reports Down with a message.
    assert_eq!(v["reranker"]["state"], "Down");
    assert!(v["reranker"]["message"].is_string());
    // Never fatal: overall status and HTTP code are untouched.
    assert_eq!(
        v["status"], "Ok",
        "a sick reranker must not flip overall status"
    );
    assert_eq!(status, StatusCode::OK, "and must not 503 the endpoint");
}

// Sprint 040 (#791) regression. The `/healthz` reranker probe caches its
// verdict for 2 s, and that cache used to be keyed on the reranker's
// base URL. A URL is not an identity: an ephemeral port is recycled the
// instant its listener drops, so a fresh mock server routinely binds the
// URL a just-dropped one had and inherits a verdict about a server that
// no longer exists.
//
// That is what made the two tests above flaky — sequentially (which is
// what CI's core count sometimes forces) the sick server's port was
// handed straight to the healthy one, and a server answering 200 was
// reported `Down`. It reproduced 60/60 under `--test-threads=1`.
//
// This test pins the bug without depending on port luck: one server, one
// URL, two reranker instances, behaviour changed in between. Under the
// old URL-keyed cache the second probe returns the first's stale `Ok`.
#[tokio::test]
async fn reranker_probe_cache_does_not_leak_between_instances_sharing_a_url() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let healthy = Arc::new(klams_store::TeiReranker::new(server.uri()).unwrap());
    let (_, v) = healthz_json(router_with_reranker(Some(healthy))).await;
    assert_eq!(v["reranker"]["state"], "Ok", "sanity: the 200 server is Ok");

    // Same URL, a different instance, now sick — and well inside the 2 s
    // cache TTL, which is the whole point.
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let sick = Arc::new(klams_store::TeiReranker::new(server.uri()).unwrap());
    let (_, v) = healthz_json(router_with_reranker(Some(sick))).await;
    assert_eq!(
        v["reranker"]["state"], "Down",
        "a second instance must be probed on its own merits, not served \
         the previous instance's cached verdict for the same URL"
    );
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

#[tokio::test]
async fn healthz_includes_inactive_maintenance_block() {
    let app = router();
    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let m = &v["maintenance"];
    assert_eq!(m["active"], false);
    assert!(m.get("run_id").is_none() || m["run_id"].is_null());
    assert!(m.get("started_at").is_none() || m["started_at"].is_null());
}

#[tokio::test]
async fn healthz_includes_active_maintenance_block_with_run_id() {
    use klams_types::{MaintenanceState, RunningSnapshot};
    use ulid::Ulid;

    let (queue, _rx) = MemoryQueue::new(8);
    let maintenance = MaintenanceState::new();
    let run_id = Ulid::new();
    let started_at = chrono::Utc::now();
    maintenance.mark_active(RunningSnapshot {
        run_id,
        started_at,
        expected_end_at: Some(started_at + chrono::Duration::seconds(120)),
    });
    let state = ApiState {
        store: Arc::new(HealthyStore),
        queue,
        api: klams_types::ApiConfig::default(),
        queue_capacity: 8,
        workers: 2,
        started_at: std::time::Instant::now(),
        validators: std::sync::Arc::new(klams_core::ValidatorRegistry::with_defaults()),
        context_builder: std::sync::Arc::new(klams_core::context::ContextBuilder::new(
            klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
            100,
        )),
        maintenance,
        embed_limit: klams_types::EmbedLimit::default(),
        fusion: klams_types::FusionStrategy::default_rrf(),
        reranker: None,
        rerank_window: 50,
    };
    let app = build_router(state, "test-token");

    let req = Request::builder()
        .method(Method::GET)
        .uri("/healthz")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();

    let m = &v["maintenance"];
    assert_eq!(m["active"], true);
    assert_eq!(m["run_id"], run_id.to_string());
    assert!(m["started_at"].is_string());
    assert!(m["expected_end_at"].is_string());
}
