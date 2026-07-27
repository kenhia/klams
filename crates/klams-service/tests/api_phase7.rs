//! Sprint 007 Phase 7 (US5) — REST `/v1/authors` drilldown smoke tests
//! against the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::{
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs},
    memory_append_event::{run as append_event, MemoryAppendEventArgs},
    register_author::{run as register, RegisterAuthorInput},
    McpState,
};
use klams_types::MaintenanceState;
use serde_json::Value;
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
            model: Some("test-model".into()),
            session_title: Some("phase-7".into()),
            repo: Some("/tmp/test".into()),
            client_app: None,
            client_version: None,
            extra: Value::Null,
        },
    )
    .await
    .expect("register_author")
    .author_id
}

async fn http_get(server: &TestServer, path: &str) -> (reqwest::StatusCode, Value) {
    let url = format!("http://{}{}", server.addr, path);
    let resp = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&server.bearer_token)
        .send()
        .await
        .expect("send");
    let status = resp.status();
    let body: Value = resp.json().await.expect("json body");
    (status, body)
}

/// A unique `EnvFact` key in the shape the validator requires
/// (`^[A-Z][A-Z0-9_]*$`).
///
/// Sprint 031 (#645): these seeds used to be `phase7-list-<uuid>`, which
/// REST has always rejected and MCP accepted, because the MCP write path
/// ran no validation at all. With both surfaces on one
/// `ValidatorRegistry` the old spelling fails — correctly.
fn env_key(prefix: &str, uniq: Uuid) -> String {
    format!("{prefix}_{}", uniq.simple().to_string().to_uppercase())
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_list_returns_registered_author_with_counts() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "ghcp-phase7-list").await;
    let uniq = Uuid::now_v7();

    // Write one fact + one event so counts are non-zero.
    memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": env_key("PHASE7_LIST", uniq), "value": "x"}),
        ),
    )
    .await
    .expect("fact");
    append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author,
            category: "test.phase7".into(),
            payload: serde_json::json!({"step": 1}),
            task_id: None,
        },
    )
    .await
    .expect("event");

    let (status, body) =
        http_get(&server, "/v1/authors?agent_name=ghcp-phase7-list&limit=50").await;
    assert_eq!(status, 200);
    let authors = body["authors"].as_array().expect("authors array");
    let row = authors
        .iter()
        .find(|a| a["id"].as_str() == Some(&author.to_string()))
        .expect("author in list");
    let counts = &row["counts"];
    assert!(counts["writes"].as_i64().unwrap_or(0) >= 1);
    assert!(counts["events"].as_i64().unwrap_or(0) >= 1);
    assert_eq!(counts["soft_deletes"].as_i64(), Some(0));
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_detail_returns_404_for_unknown() {
    let server = TestServer::spawn().await;
    let unknown = Uuid::now_v7();
    let (status, _) = http_get(&server, &format!("/v1/authors/{unknown}")).await;
    assert_eq!(status, 404);
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_detail_returns_author_projection() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "ghcp-phase7-detail").await;
    let (status, body) = http_get(&server, &format!("/v1/authors/{author}")).await;
    assert_eq!(status, 200);
    assert_eq!(body["id"].as_str(), Some(author.to_string().as_str()));
    assert_eq!(body["agent_name"].as_str(), Some("ghcp-phase7-detail"));
    assert!(body["counts"].is_object());
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_memories_lists_facts_and_events() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "ghcp-phase7-memories").await;
    let uniq = Uuid::now_v7();

    let fact = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": env_key("PHASE7_MEM", uniq), "value": "y"}),
        ),
    )
    .await
    .expect("fact");
    append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author,
            category: "test.phase7.mem".into(),
            payload: serde_json::json!({"step": 1}),
            task_id: None,
        },
    )
    .await
    .expect("event");

    let mut memories: Vec<serde_json::Value> = Vec::new();
    let mut path = format!("/v1/authors/{author}/memories?kinds=fact,event&state=live&limit=50");
    loop {
        let (status, body) = http_get(&server, &path).await;
        assert_eq!(status, 200);
        for m in body["memories"].as_array().expect("memories") {
            memories.push(m.clone());
        }
        let Some(cursor) = body["next_cursor"].as_str() else {
            break;
        };
        path = format!(
            "/v1/authors/{author}/memories?kinds=fact,event&state=live&limit=50&cursor={cursor}"
        );
    }
    assert!(
        memories
            .iter()
            .any(|m| m["id"].as_str() == Some(&fact.id.to_string())),
        "fact should appear in memories list"
    );
    assert!(
        memories.iter().any(|m| m["kind"].as_str() == Some("event")),
        "event should appear in memories list"
    );
    // All live rows.
    for m in &memories {
        assert_eq!(m["state"].as_str(), Some("live"));
    }
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_memories_bad_state_returns_400() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "ghcp-phase7-badstate").await;
    let (status, _) = http_get(
        &server,
        &format!("/v1/authors/{author}/memories?state=bogus"),
    )
    .await;
    assert_eq!(status, 400);
}
