//! US2 dissent lifecycle e2e tests (T023).
//!
//! Runs against the docker-compose test stack. Marked `#[ignore]`
//! so plain `cargo test` skips it; invoke with
//! `cargo test -p klams-service --test us2_dissents -- --ignored`.

mod common;

use common::TestServer;
use klams_types::{FactType, FactWriteOutcome, Source, UpsertFactRequest};
use serde_json::json;
use uuid::Uuid;

fn req_user(name: &str, nonce: &str) -> UpsertFactRequest {
    UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"name": name, "nonce": nonce}),
        source: Source::User,
        explicit_id: None,
        expected_version: None,
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn dissent_lifecycle_promote() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().to_string();
    let fact = match server
        .client
        .upsert_fact(&req_user("Ada", &nonce))
        .await
        .unwrap()
    {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted, got {other:?}"),
    };

    let contradicting = UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"name": "Grace", "nonce": nonce.clone()}),
        source: Source::AgentProposal,
        explicit_id: Some(fact.id),
        expected_version: None,
    };
    let dissent_id = match server.client.upsert_fact(&contradicting).await.unwrap() {
        FactWriteOutcome::Dissented { dissent_id, .. } => dissent_id,
        other => panic!("expected Dissented, got {other:?}"),
    };

    let promoted = server
        .client
        .promote_dissent(dissent_id, Source::User, fact.version)
        .await
        .expect("promote");
    assert!(promoted.version > fact.version);
    assert_eq!(promoted.payload["name"], "Grace");
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn dissent_dedupe_path() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().to_string();
    let fact = match server
        .client
        .upsert_fact(&req_user("Ada", &nonce))
        .await
        .unwrap()
    {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted, got {other:?}"),
    };
    let bad = UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"name": "Grace", "nonce": nonce.clone()}),
        source: Source::AgentProposal,
        explicit_id: Some(fact.id),
        expected_version: None,
    };
    let id1 = match server.client.upsert_fact(&bad).await.unwrap() {
        FactWriteOutcome::Dissented { dissent_id, .. } => dissent_id,
        other => panic!("expected Dissented, got {other:?}"),
    };
    let id2 = match server.client.upsert_fact(&bad).await.unwrap() {
        FactWriteOutcome::Dissented { dissent_id, .. } => dissent_id,
        other => panic!("expected Dissented, got {other:?}"),
    };
    assert_eq!(id1, id2, "dedupe must return the same dissent id");
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn dissent_discard_marks_resolved() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().to_string();
    let fact = match server
        .client
        .upsert_fact(&req_user("Ada", &nonce))
        .await
        .unwrap()
    {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted, got {other:?}"),
    };
    let bad = UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"name": "Grace", "nonce": nonce.clone()}),
        source: Source::AgentProposal,
        explicit_id: Some(fact.id),
        expected_version: None,
    };
    let dissent_id = match server.client.upsert_fact(&bad).await.unwrap() {
        FactWriteOutcome::Dissented { dissent_id, .. } => dissent_id,
        other => panic!("expected Dissented, got {other:?}"),
    };
    let resolved = server
        .client
        .discard_dissent(dissent_id, Source::Controller)
        .await
        .expect("discard");
    assert_eq!(resolved.status, klams_types::DissentStatus::Discarded);
}
