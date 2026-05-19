//! US1 / sprint-003 T017 — Ansible-originated facts (`source=Task` + `task_id`).
//!
//! Three acceptance scenarios from `spec.md` US1:
//!   1. A Task-source fact carrying `task_id` lands canonical with the
//!      `task_id` preserved in the payload.
//!   2. Re-submitting the identical payload bumps no version (dedupe holds).
//!   3. A second Task-source fact targeting the same canonical fact id but
//!      lower trust than an existing User-source row gets diverted to
//!      dissents (policy enforced for Task vs User).
//!
//! Run with the docker test stack up:
//!   `cargo test -p klams-service --test us3b_ansible_facts -- --ignored --test-threads=1`

mod common;

use common::TestServer;
use klams_types::{FactType, FactWriteOutcome, Source, UpsertFactRequest};
use serde_json::json;
use uuid::Uuid;

fn ansible_env_fact(host: &str, value: &str, task_id: &str) -> UpsertFactRequest {
    UpsertFactRequest {
        fact_type: FactType::EnvFact,
        payload: json!({
            "key": "GPU_COUNT",
            "value": value,
            "host": host,
            "task_id": task_id,
        }),
        source: Source::Task,
        explicit_id: None,
        expected_version: None,
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn ansible_task_fact_lands_canonical() {
    let server = TestServer::spawn().await;
    let run_id = format!("ansible-{}", "0".repeat(32));
    let nonce = Uuid::now_v7().to_string();
    let req = UpsertFactRequest {
        fact_type: FactType::EnvFact,
        payload: json!({
            "key": "GPU_COUNT",
            "value": "2",
            "host": "kubs0",
            "task_id": run_id,
            "nonce": nonce,
        }),
        source: Source::Task,
        explicit_id: None,
        expected_version: None,
    };
    let fact = match server.client.upsert_fact(&req).await.expect("upsert") {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted, got {other:?}"),
    };
    assert_eq!(fact.source, Source::Task);
    assert_eq!(fact.payload["task_id"], run_id);
    assert_eq!(fact.version, 1);
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn ansible_task_fact_rerun_no_new_version() {
    let server = TestServer::spawn().await;
    let run_id = format!("ansible-{}", "1".repeat(32));
    let host = format!("kubs-{}", Uuid::now_v7().simple());
    let req = ansible_env_fact(&host, "8", &run_id);

    let first = match server.client.upsert_fact(&req).await.expect("first") {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted, got {other:?}"),
    };
    let second = match server.client.upsert_fact(&req).await.expect("second") {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted on dedupe rerun, got {other:?}"),
    };
    assert_eq!(first.id, second.id, "same fact id on rerun");
    assert_eq!(first.version, second.version, "version must not bump");
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn ansible_task_write_diverts_against_user_canonical() {
    let server = TestServer::spawn().await;
    let host = format!("kubs-{}", Uuid::now_v7().simple());

    // Seed a User-source EnvFact for this host/key.
    let user_req = UpsertFactRequest {
        fact_type: FactType::EnvFact,
        payload: json!({"key": "GPU_COUNT", "value": "4", "host": host}),
        source: Source::User,
        explicit_id: None,
        expected_version: None,
    };
    let user_fact = match server.client.upsert_fact(&user_req).await.expect("seed") {
        FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("expected Persisted seed, got {other:?}"),
    };

    // Ansible-style Task write targeting the same canonical row id with a
    // contradicting value. Task < User → must divert to dissents.
    let run_id = format!("ansible-{}", "2".repeat(32));
    let contradicting = UpsertFactRequest {
        fact_type: FactType::EnvFact,
        payload: json!({
            "key": "GPU_COUNT",
            "value": "8",
            "host": host,
            "task_id": run_id,
        }),
        source: Source::Task,
        explicit_id: Some(user_fact.id),
        expected_version: None,
    };
    match server
        .client
        .upsert_fact(&contradicting)
        .await
        .expect("dissent")
    {
        FactWriteOutcome::Dissented { dissent_id, .. } => {
            assert!(!dissent_id.is_nil());
        }
        other => panic!("expected Dissented, got {other:?}"),
    }
}
