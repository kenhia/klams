//! Sprint 007 Phase 5 — `memory_append_event` end-to-end against
//! the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::{
    memory_append_event::{run as append_event, MemoryAppendEventArgs},
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
        Arc::new(vec![]),
    )
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_append_event_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-event".into(),
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

    let task_id = Uuid::now_v7();
    let mem = append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author.author_id,
            category: "deploy.completed".into(),
            payload: serde_json::json!({
                "service": "klams-service",
                "version": "0.7.0",
                "env": "prod"
            }),
            task_id: Some(task_id),
        },
    )
    .await
    .expect("memory_append_event");
    assert_eq!(mem.kind, MemoryKind::Event);
    assert_eq!(mem.author.agent_name, "GHCP-test-event");
    match mem.content {
        PublicMemoryContent::Event {
            category,
            payload,
            task_id: tid,
        } => {
            assert_eq!(category, "deploy.completed");
            assert_eq!(payload["service"], "klams-service");
            assert_eq!(tid, Some(task_id));
        }
        _ => panic!("expected event content"),
    }

    // UNKNOWN_AUTHOR_ID path.
    let err = append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: Uuid::now_v7(),
            category: "deploy.completed".into(),
            payload: serde_json::json!({}),
            task_id: None,
        },
    )
    .await
    .expect_err("expected UNKNOWN_AUTHOR_ID");
    assert_eq!(err.meta.error_code, "UNKNOWN_AUTHOR_ID");

    // INVALID_CATEGORY path (empty category).
    let err = append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author.author_id,
            category: "   ".into(),
            payload: serde_json::json!({}),
            task_id: None,
        },
    )
    .await
    .expect_err("expected INVALID_CATEGORY");
    assert_eq!(err.meta.error_code, "INVALID_CATEGORY");

    // FR-015: events are append-only; memory_delete is wired up in
    // phase 6 and will return EVENTS_NOT_DELETABLE for this id.
}
