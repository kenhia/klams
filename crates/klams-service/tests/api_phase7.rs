//! Sprint 007 Phase 7 (US5) — REST `/v1/authors` drilldown smoke tests
//! against the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::{make_author, mcp_state_from, TestServer};
use klams_mcp::tools::{
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs},
    memory_append_event::{run as append_event, MemoryAppendEventArgs},
};
use serde_json::Value;
use uuid::Uuid;

async fn http_get(server: &TestServer, path: &str) -> (reqwest::StatusCode, Value) {
    let url = format!("http://{}{}", server.addr, path);
    let resp = reqwest::Client::new()
        .get(&url)
        .header("X-Homelab-Agent", &server.full_agent)
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

/// Sprint 055 (#3079): the author route is one newest-first timeline
/// across kinds, not three kind sections in a fixed order. Written
/// event → fact → knowledge, so the old sectioned route led with the
/// fact and put the knowledge (the newest row) last.
#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_memories_is_newest_first_across_kinds() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "ghcp-055-timeline").await;
    let uniq = Uuid::now_v7();

    let event = append_event(
        &state,
        MemoryAppendEventArgs {
            author_id: author,
            category: "test.055.timeline".into(),
            payload: serde_json::json!({"step": 1}),
            task_id: None,
        },
    )
    .await
    .expect("event");
    let fact = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": env_key("S055_TIMELINE", uniq), "value": "z"}),
        ),
    )
    .await
    .expect("fact");
    let knowledge = memory_add(
        &state,
        MemoryAddArgs::knowledge(author, format!("sprint 055 author timeline probe {uniq}")),
    )
    .await
    .expect("knowledge");

    // limit=2 walks the cursor across the kind boundary too.
    let mut ids: Vec<String> = Vec::new();
    let mut path = format!("/v1/authors/{author}/memories?limit=2");
    loop {
        let (status, body) = http_get(&server, &path).await;
        assert_eq!(status, 200, "{body}");
        for m in body["memories"].as_array().expect("memories") {
            ids.push(m["id"].as_str().expect("id").to_string());
        }
        let Some(cursor) = body["next_cursor"].as_str() else {
            break;
        };
        path = format!("/v1/authors/{author}/memories?limit=2&cursor={cursor}");
    }
    // `make_author` is idempotent on the agent name, so a long-lived stack
    // can hold this author's rows from an earlier run — all older than
    // these three, so they can only trail.
    assert_eq!(
        ids.get(..3),
        Some(
            &[
                knowledge.id.to_string(),
                fact.id.to_string(),
                event.id.to_string()
            ][..]
        ),
        "one newest-first timeline across kinds: {ids:?}"
    );
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "no row twice across pages: {ids:?}"
    );
}

/// Sprint 055 (#3079): a cursor from the old sectioned route
/// (`base64("f:<ns>:<uuid>")`) is refused, not quietly re-read as a
/// merged keyset that would return the wrong page.
#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn authors_memories_old_cursor_returns_400() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "ghcp-055-oldcursor").await;
    let old = URL_SAFE_NO_PAD.encode(format!("k:0:{}", Uuid::now_v7()));
    let (status, body) = http_get(
        &server,
        &format!("/v1/authors/{author}/memories?cursor={old}"),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, _) = http_get(
        &server,
        &format!("/v1/authors/{author}/memories?cursor=not-base64!"),
    )
    .await;
    assert_eq!(status, 400);
}
