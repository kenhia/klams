//! Sprint 007 Phase 4 — `memory_search` + `memory_related` end-to-end
//! against the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::{
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs, MemoryAddContent},
    memory_related::{run as memory_related, MemoryRelatedArgs},
    memory_search::{run as memory_search, MemoryKindFilter, MemorySearchArgs},
    register_author::{run as register, RegisterAuthorInput},
    McpState,
};
use klams_types::{MaintenanceState, MemoryKind};
use std::sync::Arc;

fn mcp_state_from(server: &TestServer) -> McpState {
    McpState::new(
        Arc::clone(&server.store),
        Arc::new(MaintenanceState::default()),
        Arc::new(vec![]),
    )
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_search_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-search".into(),
            model: None,
            session_title: None,
            repo: None,
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        },
    )
    .await
    .expect("register_author");

    // Seed a fact + a knowledge item so both backends contribute.
    memory_add(
        &state,
        MemoryAddArgs {
            author_id: author.author_id,
            content: MemoryAddContent::Fact {
                fact_type: FactTypeArg::EnvFact,
                payload: serde_json::json!({
                    "key": "phase4-search-target",
                    "value": "needle phase4 search"
                }),
            },
        },
    )
    .await
    .expect("seed fact");

    memory_add(
        &state,
        MemoryAddArgs {
            author_id: author.author_id,
            content: MemoryAddContent::Knowledge {
                text: "phase4 search needle knowledge body".into(),
                tags: vec!["phase4".into()],
                source_path: None,
                repo: None,
            },
        },
    )
    .await
    .expect("seed knowledge");

    let hits = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle".into(),
            kinds: None,
            tags: None,
            top_k: Some(20),
        },
    )
    .await
    .expect("memory_search");
    assert!(!hits.is_empty(), "expected at least one search hit");
    // FR-011: projection must not leak internal fields. PublicMemory
    // has no version / confidence / decay_weight by construction;
    // round-trip through serde and confirm.
    let json = serde_json::to_value(&hits[0]).expect("serialize");
    let obj = json.as_object().expect("object");
    for forbidden in [
        "version",
        "confidence",
        "decay_weight",
        "use_count",
        "deleted_at",
    ] {
        assert!(
            !obj.contains_key(forbidden),
            "PublicMemory leaked `{forbidden}`"
        );
    }

    // Kind filter narrows the result.
    let only_knowledge = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle".into(),
            kinds: Some(vec![MemoryKindFilter::Knowledge]),
            tags: None,
            top_k: Some(20),
        },
    )
    .await
    .expect("memory_search knowledge-only");
    assert!(only_knowledge
        .iter()
        .all(|m| m.kind == MemoryKind::Knowledge));

    // Tag filter is honored.
    let tagged = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle".into(),
            kinds: Some(vec![MemoryKindFilter::Knowledge]),
            tags: Some(vec!["phase4".into()]),
            top_k: Some(20),
        },
    )
    .await
    .expect("memory_search tagged");
    assert!(tagged.iter().all(|m| m.tags.iter().any(|t| t == "phase4")));
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_related_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-related".into(),
            model: None,
            session_title: None,
            repo: None,
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        },
    )
    .await
    .expect("register_author");

    // Seed several similar knowledge points.
    let mut ids = Vec::new();
    for i in 0..4 {
        let mem = memory_add(
            &state,
            MemoryAddArgs {
                author_id: author.author_id,
                content: MemoryAddContent::Knowledge {
                    text: format!("phase4 related seed point number {i} about kittens"),
                    tags: vec!["related-seed".into()],
                    source_path: None,
                    repo: None,
                },
            },
        )
        .await
        .expect("seed knowledge");
        ids.push(mem.id);
    }

    let neighbours = memory_related(
        &state,
        MemoryRelatedArgs {
            id: ids[0],
            top_k: Some(3),
        },
    )
    .await
    .expect("memory_related");
    assert!(neighbours.len() <= 3, "respects top_k");
    assert!(
        neighbours.iter().all(|m| m.id != ids[0]),
        "seed point excluded from results"
    );
    assert!(
        neighbours.iter().all(|m| m.kind == MemoryKind::Knowledge),
        "only knowledge returned"
    );
}
