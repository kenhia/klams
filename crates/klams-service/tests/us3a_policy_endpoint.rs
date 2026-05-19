//! US5 / sprint-003 T015 — `/memory/policy` + `klams_writes_total` counter.
//!
//! Three sub-cases (all `#[ignore]`, run with `cargo test -p klams-service
//! --test us3a_policy_endpoint -- --ignored --test-threads=1`):
//!
//! 1. `policy_endpoint_returns_default_table` — endpoint serves the same
//!    `PolicyTable` the dispatcher reads from (FR-018 / SC-005).
//! 2. `writes_total_canonical_increments_after_user_fact` — a user fact
//!    bumps `klams_writes_total{path="canonical"}`.
//! 3. `writes_total_dissent_increments_after_diverted_write` — an
//!    agent-proposed contradiction bumps `klams_writes_total{path="dissent"}`.
//!
//! Pulls the live recorder render to check counter labels.

mod common;

use common::TestServer;
use klams_core::PolicyTable;
use klams_types::{FactType, FactWriteOutcome, Source, UpsertFactRequest};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use serde_json::json;
use std::sync::OnceLock;
use uuid::Uuid;

static RECORDER: OnceLock<PrometheusHandle> = OnceLock::new();

fn install_recorder() -> &'static PrometheusHandle {
    RECORDER.get_or_init(|| {
        let handle = PrometheusBuilder::new()
            .install_recorder()
            .expect("install prometheus recorder");
        klams_core::metrics::describe();
        handle
    })
}

fn req_user(name: &str, nonce: &str) -> UpsertFactRequest {
    UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"name": name, "nonce": nonce}),
        source: Source::User,
        explicit_id: None,
        expected_version: None,
    }
}

/// Search a Prometheus text render for a `klams_writes_total{...}` line
/// matching all provided label fragments and return its current sample
/// value (0 if no matching series exists yet).
fn writes_total_sample(render: &str, labels: &[&str]) -> u64 {
    render
        .lines()
        .filter(|l| l.starts_with("klams_writes_total{"))
        .filter(|l| labels.iter().all(|frag| l.contains(frag)))
        .filter_map(|l| l.rsplit_once(' ').and_then(|(_, v)| v.parse::<u64>().ok()))
        .sum()
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn policy_endpoint_returns_default_table() {
    let server = TestServer::spawn().await;
    let got = server.client.policy().await.expect("policy");
    assert_eq!(got, PolicyTable::default());
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn writes_total_canonical_increments_after_user_fact() {
    let handle = install_recorder();
    let server = TestServer::spawn().await;

    let before = writes_total_sample(
        &handle.render(),
        &["type=\"fact\"", "source=\"user\"", "path=\"canonical\""],
    );

    let nonce = Uuid::now_v7().to_string();
    let res = server
        .client
        .upsert_fact(&req_user("Ada", &nonce))
        .await
        .expect("upsert");
    match res {
        FactWriteOutcome::Persisted { .. } => {}
        other => panic!("expected Persisted, got {other:?}"),
    }

    let after = writes_total_sample(
        &handle.render(),
        &["type=\"fact\"", "source=\"user\"", "path=\"canonical\""],
    );
    assert!(
        after > before,
        "klams_writes_total canonical did not increment (before={before}, after={after})"
    );
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn writes_total_dissent_increments_after_diverted_write() {
    let handle = install_recorder();
    let server = TestServer::spawn().await;

    let nonce = Uuid::now_v7().to_string();
    let fact = match server
        .client
        .upsert_fact(&req_user("Ada", &nonce))
        .await
        .expect("seed")
    {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted, got {other:?}"),
    };

    let before = writes_total_sample(
        &handle.render(),
        &[
            "type=\"fact\"",
            "source=\"agent_proposal\"",
            "path=\"dissent\"",
        ],
    );

    let contradiction = UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"name": "Grace", "nonce": nonce.clone()}),
        source: Source::AgentProposal,
        explicit_id: Some(fact.id),
        expected_version: None,
    };
    match server
        .client
        .upsert_fact(&contradiction)
        .await
        .expect("dissent")
    {
        FactWriteOutcome::Dissented { .. } => {}
        other => panic!("expected Dissented, got {other:?}"),
    }

    let after = writes_total_sample(
        &handle.render(),
        &[
            "type=\"fact\"",
            "source=\"agent_proposal\"",
            "path=\"dissent\"",
        ],
    );
    assert!(
        after > before,
        "klams_writes_total dissent did not increment (before={before}, after={after})"
    );
}
