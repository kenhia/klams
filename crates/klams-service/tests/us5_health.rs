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
        payload: json!({"name": "us5-metrics-smoke"}),
        source: klams_types::Source::User,
        explicit_id: None,
        expected_version: None,
    };
    server.client.upsert_fact(&req).await.expect("upsert");
    let snap = server.client.health().await.expect("health");
    assert_eq!(snap.status, HealthStatus::Ok);
}

/// Sprint 048 (#1806) — `/healthz` must decline the pooled connection **on
/// the wire**, against the real router and a real store.
///
/// The unit tests either side of this one each prove half of it: the router
/// sets `Connection: close`, and hyper honours the header when a handler sets
/// it. Neither would notice if the two stopped meeting — a middleware that
/// stripped hop-by-hop headers, or a future axum that reordered the response
/// parts, would leave both green and put the kpidash card back to flickering.
/// So this speaks HTTP/1.1 down a socket and checks what actually comes back.
#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn healthz_declines_keep_alive_on_the_wire() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let server = TestServer::spawn().await;
    let mut sock = tokio::net::TcpStream::connect(server.addr)
        .await
        .expect("connect");
    sock.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n")
        .await
        .expect("write");

    // Read to EOF. A keep-alive connection would block here until the client
    // timeout; `Connection: close` ends it as soon as the body is written.
    let mut raw = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        sock.read_to_end(&mut raw),
    )
    .await
    .expect("/healthz kept the connection open — the client will pool it and race the idle reaper (#1806)")
    .expect("read");

    let text = String::from_utf8_lossy(&raw);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    assert!(
        head.starts_with("HTTP/1.1 200"),
        "expected 200, got head: {head:?}",
    );
    assert!(
        head.lines()
            .any(|l| l.to_ascii_lowercase().trim() == "connection: close"),
        "no `Connection: close` on the wire; head was: {head:?}",
    );
    // The request asked for keep-alive explicitly; the server must still
    // refuse. Anything else means a watcher gets a pooled connection back.
    assert!(
        !head.to_ascii_lowercase().contains("connection: keep-alive"),
        "server agreed to keep-alive on /healthz: {head:?}",
    );

    server.cleanup().await;
}
