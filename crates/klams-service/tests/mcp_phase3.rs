//! Sprint 007 Phase 3 — `register_author` + `memory_add` end-to-end
//! against the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::memory_add::run as memory_add;
use klams_mcp::tools::{
    memory_add::{FactTypeArg, MemoryAddArgs},
    register_author::{run as register, RegisterAuthorInput},
    McpState,
};
use klams_types::{MaintenanceState, MemoryKind, PublicMemoryContent};
use std::sync::Arc;
use uuid::Uuid;

fn mcp_state_from(server: &TestServer) -> McpState {
    McpState::new(
        Arc::clone(&server.store),
        Arc::new(MaintenanceState::default()),
        klams_types::ApiConfig::default(),
    )
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn register_author_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let out = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test".into(),
            model: Some("claude-opus-4.7".into()),
            session_title: Some("phase 3 smoke".into()),
            repo: Some("/tmp/x".into()),
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        },
    )
    .await
    .expect("register_author");
    assert_eq!(out.agent_name, "GHCP-test");
    // UUID v7 has variant 0b10 and version 7 (top nibble of bytes[6] = 0x7).
    let bytes = out.author_id.as_bytes();
    assert_eq!(bytes[6] >> 4, 0x7, "expected UUID v7");
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_add_fact_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-fact".into(),
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

    let mem = memory_add(
        &state,
        MemoryAddArgs::fact(
            author.author_id,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": "phase3-smoke", "value": "ok"}),
        ),
    )
    .await
    .expect("memory_add fact");

    assert_eq!(mem.kind(), MemoryKind::Fact);
    assert_eq!(mem.author.agent_name, "GHCP-test-fact");
    match mem.content {
        PublicMemoryContent::Fact { fact_type, .. } => assert_eq!(fact_type, "EnvFact"),
        _ => panic!("expected fact content"),
    }

    // UNKNOWN_AUTHOR_ID path.
    let err = memory_add(
        &state,
        MemoryAddArgs::fact(
            Uuid::now_v7(),
            FactTypeArg::EnvFact,
            serde_json::json!({"k": 1}),
        ),
    )
    .await
    .expect_err("expected UNKNOWN_AUTHOR_ID");
    assert_eq!(err.meta.error_code, "UNKNOWN_AUTHOR_ID");
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_add_knowledge_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-know".into(),
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

    let mem = memory_add(
        &state,
        MemoryAddArgs {
            tags: vec!["phase3".into()],
            source_path: Some("/tmp/x.md".into()),
            repo: Some("/tmp/x".into()),
            ..MemoryAddArgs::knowledge(author.author_id, "klams-mcp phase 3 smoke knowledge point")
        },
    )
    .await
    .expect("memory_add knowledge");
    assert_eq!(mem.kind(), MemoryKind::Knowledge);
    assert_eq!(mem.author.agent_name, "GHCP-test-know");
}
