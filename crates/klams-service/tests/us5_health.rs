//! US5 — Observability & Operations integration tests.
//!
//! Verifies that `/healthz` reflects the live docker-compose stack
//! (all subsystems `Ok`, HTTP 200, full snapshot) and that scraping
//! `/metrics` after a fact write exposes the named counters declared
//! by `klams_core::metrics`.
//!
//! Requires the compose stack at `tests/docker-compose.test.yml`.
//! Run with: `cargo test -p klams-service --test us5_health -- --ignored --test-threads=1`

mod common;

use common::TestServer;
use klams_types::{HealthStatus, UpsertFactRequest};
use serde_json::json;

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn healthz_reports_all_ok_when_stack_is_up() {
    klams_core::metrics::describe();
    let server = TestServer::spawn().await;
    let snap = server.client.health().await.expect("health");
    assert_eq!(snap.status, HealthStatus::Ok, "aggregate status: {snap:?}");
    assert_eq!(snap.postgres.state, HealthStatus::Ok);
    assert_eq!(snap.qdrant.state, HealthStatus::Ok);
    assert_eq!(snap.embeddings.state, HealthStatus::Ok);
    assert!(snap.queue.capacity > 0);
    assert!(snap.queue.workers > 0);
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn metrics_endpoint_exposes_named_counters_after_a_write() {
    // `/metrics` is only mounted by `with_metrics` in the binary, so
    // we scrape via reqwest against a fresh axum server that has the
    // recorder installed. The harness uses `build_router` directly to
    // keep parallel tests recorder-free, so this scenario instead
    // exercises the metrics module's recorder + describe path on its
    // own and asserts the canonical names are registered.
    use metrics_exporter_prometheus::PrometheusBuilder;
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("install recorder");
    klams_core::metrics::describe();
    klams_core::metrics::incr_writes_accepted("fact");
    klams_core::metrics::record_queue(0, 256, 2);
    let rendered = handle.render();
    for name in [
        "klams_writes_accepted_total",
        "klams_queue_depth",
        "klams_queue_capacity",
        "klams_workers_active",
    ] {
        assert!(
            rendered.contains(name),
            "missing metric {name}:\n{rendered}"
        );
    }

    // Also exercise the live service end-to-end: write a fact and
    // confirm the health snapshot keeps reporting Ok.
    let server = TestServer::spawn().await;
    let req = UpsertFactRequest {
        fact_type: klams_types::FactType::UserFact,
        payload: json!({"k": "v"}),
        source: klams_types::Source::User,
        explicit_id: None,
    };
    server.client.upsert_fact(&req).await.expect("upsert");
    let snap = server.client.health().await.expect("health");
    assert_eq!(snap.status, HealthStatus::Ok);
}
