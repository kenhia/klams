//! Sprint 015 — `dissent_propose` end-to-end against the in-process
//! docker compose stack: propose → pending dissent with
//! reason/author, dedupe on identical payload, `NOT_FOUND` on missing
//! or soft-deleted facts, and the existing discard flow still applies.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::{
    dissent_propose::{run as dissent_propose, DissentProposeArgs},
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs},
    memory_delete::{run as memory_delete, DeleteCaller, MemoryDeleteArgs},
    register_author::{run as register, RegisterAuthorInput},
    McpState,
};
use klams_types::{DissentStatus, MaintenanceState, Source};
use std::sync::Arc;
use uuid::Uuid;

/// Sprint 025 (#633): these tests delete memories they authored
/// themselves, so a plain write-scoped caller bound to that author is
/// the right identity — no `manage` needed.
fn owner_caller(author_id: Uuid) -> DeleteCaller {
    DeleteCaller {
        author_id,
        scopes: vec![klams_types::Scope::Read, klams_types::Scope::Write],
    }
}

fn mcp_state_from(server: &TestServer) -> McpState {
    McpState::new(
        Arc::clone(&server.store),
        Arc::new(MaintenanceState::default()),
        klams_types::ApiConfig::default(),
    )
}

async fn make_author(state: &McpState, name: &str) -> Uuid {
    register(
        state,
        RegisterAuthorInput {
            agent_name: name.into(),
            model: None,
            session_title: None,
            repo: None,
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        },
    )
    .await
    .expect("register_author")
    .author_id
}

async fn seed_fact(state: &McpState, author: Uuid, key: &str, value: &str) -> Uuid {
    memory_add(
        state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({ "key": key, "value": value }),
        ),
    )
    .await
    .expect("memory_add fact")
    .id
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn propose_creates_pending_dissent_with_provenance() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "klams-mind-test").await;
    let fact_id = seed_fact(&state, author, "sprint015-propose", "old-value").await;
    let contradicting = Uuid::now_v7();

    let out = dissent_propose(
        &state,
        DissentProposeArgs {
            author_id: author,
            fact_id,
            proposed_payload: serde_json::json!({
                "key": "sprint015-propose", "value": "corrected-value"
            }),
            reason: "value contradicts a newer memory".into(),
            contradicting_memory_id: Some(contradicting),
        },
    )
    .await
    .expect("dissent_propose");
    assert_eq!(out.fact_id, fact_id);
    assert_eq!(out.status, "pending");
    assert!(!out.deduped, "first proposal must not dedupe");

    // The dissent is visible through the operator read path with the
    // proposal provenance recorded.
    let d = server
        .store
        .postgres
        .get_dissent(out.dissent_id)
        .await
        .expect("get_dissent")
        .expect("dissent exists");
    assert_eq!(d.status, DissentStatus::Pending);
    assert_eq!(d.source, Source::AgentProposal);
    assert_eq!(d.author_id, Some(author));
    assert_eq!(d.contradicting_memory_id, Some(contradicting));
    assert_eq!(
        d.reason.as_deref(),
        Some("value contradicts a newer memory")
    );

    // Identical payload dedupes into the same pending dissent.
    let again = dissent_propose(
        &state,
        DissentProposeArgs {
            author_id: author,
            fact_id,
            proposed_payload: serde_json::json!({
                "key": "sprint015-propose", "value": "corrected-value"
            }),
            reason: "different wording, same correction".into(),
            contradicting_memory_id: None,
        },
    )
    .await
    .expect("dissent_propose dedupe");
    assert_eq!(again.dissent_id, out.dissent_id);
    assert!(again.deduped, "identical payload must dedupe");
    let d = server
        .store
        .postgres
        .get_dissent(out.dissent_id)
        .await
        .expect("get_dissent")
        .expect("dissent exists");
    assert_eq!(d.submission_count, 2);
    // Original reason is kept on dedupe.
    assert_eq!(
        d.reason.as_deref(),
        Some("value contradicts a newer memory")
    );

    // The existing human resolution flow applies unchanged.
    let discarded = server
        .store
        .postgres
        .discard_dissent(out.dissent_id, Source::User)
        .await
        .expect("discard_dissent");
    assert_eq!(discarded.status, DissentStatus::Discarded);
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn propose_rejects_missing_and_deleted_facts() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "klams-mind-test-nf").await;

    // Unknown fact id → NOT_FOUND.
    let err = dissent_propose(
        &state,
        DissentProposeArgs {
            author_id: author,
            fact_id: Uuid::now_v7(),
            proposed_payload: serde_json::json!({ "key": "x", "value": "y" }),
            reason: "n/a".into(),
            contradicting_memory_id: None,
        },
    )
    .await
    .expect_err("unknown fact must error");
    assert_eq!(err.meta.error_code, "NOT_FOUND", "{err:?}");

    // Soft-deleted fact → NOT_FOUND.
    let fact_id = seed_fact(&state, author, "sprint015-deleted", "v").await;
    memory_delete(
        &state,
        MemoryDeleteArgs {
            author_id: None,
            id: fact_id,
        },
        Some(&owner_caller(author)),
    )
    .await
    .expect("memory_delete");
    let err = dissent_propose(
        &state,
        DissentProposeArgs {
            author_id: author,
            fact_id,
            proposed_payload: serde_json::json!({ "key": "sprint015-deleted", "value": "w" }),
            reason: "n/a".into(),
            contradicting_memory_id: None,
        },
    )
    .await
    .expect_err("soft-deleted fact must error");
    assert_eq!(err.meta.error_code, "NOT_FOUND", "{err:?}");
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn propose_validates_reason_and_payload() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "klams-mind-test-val").await;
    let fact_id = seed_fact(&state, author, "sprint015-validate", "v").await;

    // Empty reason.
    let err = dissent_propose(
        &state,
        DissentProposeArgs {
            author_id: author,
            fact_id,
            proposed_payload: serde_json::json!({ "key": "sprint015-validate", "value": "w" }),
            reason: "   ".into(),
            contradicting_memory_id: None,
        },
    )
    .await
    .expect_err("blank reason must error");
    assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED", "{err:?}");

    // Non-object payload.
    let err = dissent_propose(
        &state,
        DissentProposeArgs {
            author_id: author,
            fact_id,
            proposed_payload: serde_json::json!("just a string"),
            reason: "valid reason".into(),
            contradicting_memory_id: None,
        },
    )
    .await
    .expect_err("non-object payload must error");
    assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED", "{err:?}");
}
