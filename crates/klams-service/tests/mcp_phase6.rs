//! Sprint 007 Phase 6 — `memory_delete` and admin restore / hard-delete /
//! list-deleted end-to-end against the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::{
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs},
    memory_admin_hard_delete::{run as admin_hard_delete, MemoryAdminHardDeleteArgs},
    memory_admin_list_deleted::{run as admin_list_deleted, MemoryAdminListDeletedArgs},
    memory_admin_restore::{run as admin_restore, MemoryAdminRestoreArgs},
    memory_append_event::{run as append_event, MemoryAppendEventArgs},
    memory_delete::{run as memory_delete, MemoryDeleteArgs},
    memory_search::{run as memory_search, MemorySearchArgs},
    register_author::{run as register, RegisterAuthorInput},
    McpState,
};
use klams_types::MaintenanceState;
use std::sync::Arc;
use uuid::Uuid;

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

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_delete_soft_smoke() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "GHCP-phase6-delete").await;

    let fact = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({
                "key": "phase6-soft-delete",
                "value": "needle-soft-delete"
            }),
        ),
    )
    .await
    .expect("memory_add fact");

    // First delete soft-deletes.
    let out = memory_delete(
        &state,
        MemoryDeleteArgs {
            author_id: author,
            id: fact.id,
        },
    )
    .await
    .expect("memory_delete first");
    assert_eq!(out.id, fact.id);

    // Idempotent (FR-014): second call also succeeds.
    memory_delete(
        &state,
        MemoryDeleteArgs {
            author_id: author,
            id: fact.id,
        },
    )
    .await
    .expect("memory_delete idempotent");

    // Search no longer surfaces it.
    let hits = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle-soft-delete".into(),
            kinds: None,
            tags: None,
            top_k: Some(20),
        },
        None,
    )
    .await
    .expect("memory_search");
    assert!(
        hits.iter().all(|h| h.memory.id != fact.id),
        "soft-deleted fact still surfaced by search"
    );

    // Events are append-only (FR-015).
    let ev = append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author,
            category: "test.event".into(),
            payload: serde_json::json!({"k":"v"}),
            task_id: None,
        },
    )
    .await
    .expect("append_event");
    let err = memory_delete(
        &state,
        MemoryDeleteArgs {
            author_id: author,
            id: ev.id,
        },
    )
    .await
    .expect_err("expected EVENTS_NOT_DELETABLE");
    assert_eq!(err.meta.error_code, "EVENTS_NOT_DELETABLE");

    // NOT_FOUND for an unknown id.
    let err = memory_delete(
        &state,
        MemoryDeleteArgs {
            author_id: author,
            id: Uuid::now_v7(),
        },
    )
    .await
    .expect_err("expected NOT_FOUND");
    assert_eq!(err.meta.error_code, "NOT_FOUND");

    server.cleanup().await;
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_admin_restore_smoke() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "GHCP-phase6-restore").await;

    let fact = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({
                "key": "phase6-restore",
                "value": "needle-restore"
            }),
        ),
    )
    .await
    .expect("memory_add");

    // NOT_SOFT_DELETED before any delete.
    let err = admin_restore(&state, MemoryAdminRestoreArgs { id: fact.id })
        .await
        .expect_err("expected NOT_SOFT_DELETED");
    assert_eq!(err.meta.error_code, "NOT_SOFT_DELETED");

    // Soft-delete then restore.
    memory_delete(
        &state,
        MemoryDeleteArgs {
            author_id: author,
            id: fact.id,
        },
    )
    .await
    .expect("memory_delete");
    let out = admin_restore(&state, MemoryAdminRestoreArgs { id: fact.id })
        .await
        .expect("admin_restore");
    assert_eq!(out.id, fact.id);

    // Item reappears in search.
    let hits = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle-restore".into(),
            kinds: None,
            tags: None,
            top_k: Some(20),
        },
        None,
    )
    .await
    .expect("memory_search");
    assert!(
        hits.iter().any(|h| h.memory.id == fact.id),
        "restored fact missing from search"
    );

    // NOT_FOUND for unknown id.
    let err = admin_restore(&state, MemoryAdminRestoreArgs { id: Uuid::now_v7() })
        .await
        .expect_err("expected NOT_FOUND");
    assert_eq!(err.meta.error_code, "NOT_FOUND");

    server.cleanup().await;
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_admin_hard_delete_smoke() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "GHCP-phase6-hard").await;

    let fact = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({
                "key": "phase6-hard",
                "value": "needle-hard"
            }),
        ),
    )
    .await
    .expect("memory_add");

    admin_hard_delete(&state, MemoryAdminHardDeleteArgs { id: fact.id })
        .await
        .expect("admin_hard_delete");

    // Subsequent restore returns NOT_FOUND because the row is gone.
    let err = admin_restore(&state, MemoryAdminRestoreArgs { id: fact.id })
        .await
        .expect_err("expected NOT_FOUND");
    assert_eq!(err.meta.error_code, "NOT_FOUND");

    // Events still return EVENTS_NOT_DELETABLE.
    let ev = append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author,
            category: "test.event".into(),
            payload: serde_json::json!({"k":"v"}),
            task_id: None,
        },
    )
    .await
    .expect("append_event");
    let err = admin_hard_delete(&state, MemoryAdminHardDeleteArgs { id: ev.id })
        .await
        .expect_err("expected EVENTS_NOT_DELETABLE");
    assert_eq!(err.meta.error_code, "EVENTS_NOT_DELETABLE");

    server.cleanup().await;
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_admin_list_deleted_smoke() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "GHCP-phase6-list").await;

    // Seed and soft-delete several facts.
    let mut ids = Vec::new();
    for i in 0..3 {
        let f = memory_add(
            &state,
            MemoryAddArgs::fact(
                author,
                FactTypeArg::EnvFact,
                serde_json::json!({
                    "key": format!("phase6-list-{i}"),
                    "value": format!("needle-list-{i}")
                }),
            ),
        )
        .await
        .expect("memory_add");
        memory_delete(
            &state,
            MemoryDeleteArgs {
                author_id: author,
                id: f.id,
            },
        )
        .await
        .expect("memory_delete");
        ids.push(f.id);
    }

    // Page 1: limit 2.
    let page1 = admin_list_deleted(
        &state,
        MemoryAdminListDeletedArgs {
            kinds: None,
            since: None,
            author_id: Some(author),
            limit: Some(2),
            cursor: None,
        },
    )
    .await
    .expect("list page 1");
    assert_eq!(page1.results.len(), 2);
    assert!(page1.next_cursor.is_some());
    for r in &page1.results {
        assert_eq!(r.deleted_by.agent_name, "GHCP-phase6-list");
    }

    // Page 2 via cursor.
    let page2 = admin_list_deleted(
        &state,
        MemoryAdminListDeletedArgs {
            kinds: None,
            since: None,
            author_id: Some(author),
            limit: Some(2),
            cursor: page1.next_cursor.clone(),
        },
    )
    .await
    .expect("list page 2");
    // Page 2 returns the remaining row plus may transition to "k".
    let returned: std::collections::HashSet<_> = page1
        .results
        .iter()
        .chain(page2.results.iter())
        .map(|r| r.memory.id)
        .collect();
    for id in &ids {
        assert!(returned.contains(id), "missing soft-deleted id {id}");
    }

    server.cleanup().await;
}
