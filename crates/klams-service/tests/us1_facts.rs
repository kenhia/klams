//! US1: write and retrieve a fact end-to-end.
//!
//! Runs against the docker-compose test stack. Marked `#[ignore]`
//! so plain `cargo test` skips it; invoke with
//! `cargo test -p klams-service --test us1_facts -- --ignored`.

mod common;

use common::TestServer;
use klams_types::{FactType, Source, UpsertFactRequest};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_1_upsert_and_retrieve() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().to_string();

    let req = UpsertFactRequest {
        fact_type: FactType::UserFact,
        payload: json!({"key": "ram_gb", "machine": "kubs0", "nonce": nonce, "value": 64}),
        source: Source::Controller,
        explicit_id: None,
    };
    let persisted = server.client.upsert_fact(&req).await.expect("upsert");
    assert_eq!(persisted.version, 1);
    assert_eq!(persisted.payload["nonce"], nonce);

    let page = server.client.list_facts().await.expect("list");
    assert!(
        page.items.iter().any(|f| f.payload["nonce"] == nonce),
        "expected upserted fact to appear in list_facts"
    );
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_2_dedupe_by_canonical_payload() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().to_string();

    // Same logical payload, keys in different order — must hash equal
    // and upsert to the same row (version bumped only on payload change,
    // and here the payload value is byte-for-byte the same after
    // canonical sort, so version stays at 1).
    let req_a = UpsertFactRequest {
        fact_type: FactType::EnvFact,
        payload: json!({"host": "kubs0", "nonce": nonce, "ram_gb": 64}),
        source: Source::Controller,
        explicit_id: None,
    };
    let req_b = UpsertFactRequest {
        fact_type: FactType::EnvFact,
        payload: json!({"ram_gb": 64, "host": "kubs0", "nonce": nonce}),
        source: Source::Controller,
        explicit_id: None,
    };

    let first = server.client.upsert_fact(&req_a).await.expect("first");
    let second = server.client.upsert_fact(&req_b).await.expect("second");

    assert_eq!(first.id, second.id, "dedupe must return the same id");
    assert_eq!(
        second.version, first.version,
        "version must not bump on identical canonical payload"
    );
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_3_survives_restart() {
    let nonce = Uuid::now_v7().to_string();
    let (id_a, created_a) = {
        let server = TestServer::spawn().await;
        let persisted = server
            .client
            .upsert_fact(&UpsertFactRequest {
                fact_type: FactType::UserFact,
                payload: json!({"key": "persistent", "nonce": nonce}),
                source: Source::Controller,
                explicit_id: None,
            })
            .await
            .expect("upsert");
        (persisted.id, persisted.created_at)
    };

    // New TestServer = same Postgres / Qdrant via docker-compose.
    let server = TestServer::spawn().await;
    let page = server.client.list_facts().await.expect("list");
    let found = page
        .items
        .iter()
        .find(|f| f.id == id_a)
        .expect("fact survived restart");
    assert_eq!(found.created_at, created_a, "created_at must be stable");
}
