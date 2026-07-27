//! Sprint 007 T064 — regression: `/mcp` MUST require a bearer token.
//!
//! Pre-fix, `klams_api::auth::require_bearer` only sat on the REST
//! sub-router inside `build_router`, leaving `nest("/mcp", ...)`
//! unauthenticated — `initialize` and `tools/list` returned 200 to
//! anonymous callers (verified empirically on a live instance). This
//! suite locks in the unified `AuthState` mount: REST + MCP share one
//! layer.
//!
//! Marked `#[ignore]` like the rest of the integration suite — run via
//!   `cargo test -p klams-service --test mcp_auth -- --ignored`
//! after `docker compose -f tests/docker-compose.test.yml up -d`.

mod common;

use common::TestServer;

/// JSON-RPC init payload used by every probe below.
const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}"#;

async fn post_mcp(addr: std::net::SocketAddr, auth: Option<&str>) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(INIT_BODY);
    if let Some(token) = auth {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    req.send().await.expect("POST /mcp")
}

/// Parse a Streamable-HTTP SSE response body into its JSON-RPC payload.
fn parse_sse_json(body: &str) -> serde_json::Value {
    // SSE: lines like `data: {...}` separated by `\n\n`. May also be
    // bare JSON if the server picked the json_response path.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        return v;
    }
    // SSE bodies may include keep-alive `data:` pings with empty
    // payloads. Take the first `data:` line that successfully parses.
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(rest) {
                return v;
            }
        }
    }
    panic!("no JSON / data: line in body (len={}):\n{body}", body.len());
}

/// Drive a full MCP handshake (initialize + initialized notify) and
/// then call `tools/list`. Returns the list of advertised tool names.
async fn list_tools_with(addr: std::net::SocketAddr, token: &str) -> Vec<String> {
    let client = reqwest::Client::new();
    // 1. initialize
    let init = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .body(INIT_BODY)
        .send()
        .await
        .expect("initialize");
    assert_eq!(init.status(), reqwest::StatusCode::OK, "initialize ok");
    let session_id = init
        .headers()
        .get("mcp-session-id")
        .expect("mcp-session-id header")
        .to_str()
        .unwrap()
        .to_string();
    let _ = init.text().await;

    // 2. notifications/initialized (required before tools/list)
    let _ = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .header("mcp-session-id", &session_id)
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .expect("initialized notify");

    // 3. tools/list
    let resp = client
        .post(format!("http://{addr}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .header("mcp-session-id", &session_id)
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
        .send()
        .await
        .expect("tools/list");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.expect("body");
    let v = parse_sse_json(&body);
    let tools = v["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    tools
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn mcp_initialize_without_bearer_returns_401() {
    let server = TestServer::spawn().await;
    let res = post_mcp(server.addr, None).await;
    assert_eq!(
        res.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "anonymous POST /mcp must be 401"
    );
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn mcp_initialize_with_wrong_bearer_returns_401() {
    let server = TestServer::spawn().await;
    let res = post_mcp(server.addr, Some("not-the-token")).await;
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn mcp_initialize_with_valid_bearer_succeeds() {
    let server = TestServer::spawn().await;
    let token = server.bearer_token.clone();
    let res = post_mcp(server.addr, Some(&token)).await;
    assert_eq!(res.status(), reqwest::StatusCode::OK);
}

// Sprint 007 T065/T066: tools/list must be scope-filtered per FR-020.

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn tools_list_admin_sees_everything() {
    let server = TestServer::spawn().await;
    let tools = list_tools_with(server.addr, &server.bearer_token).await;
    let expected = [
        "register_author",
        "memory_add",
        "memory_search",
        "memory_related",
        "memory_append_event",
        "event_search",
        "memory_delete",
        // Sprint 029 (#638) — knowledge lifecycle verbs.
        "memory_supersede",
        "memory_update",
        // Sprint 015 added dissent_propose but missed this ignored
        // suite (not in the CI gate); expectation caught up in 018.
        "dissent_propose",
        "memory_admin_restore",
        "memory_admin_hard_delete",
        "memory_admin_list_deleted",
        // Sprint 025 (#636) — author lifecycle verbs.
        "memory_admin_list_authors",
        "memory_admin_remove_author",
        "memory_admin_merge_authors",
    ];
    for name in expected {
        assert!(
            tools.iter().any(|t| t == name),
            "admin should see {name}, got {tools:?}"
        );
    }
    assert_eq!(tools.len(), expected.len(), "no extra tools: {tools:?}");
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn tools_list_read_only_sees_only_read_tools() {
    let server = TestServer::spawn().await;
    let tools = list_tools_with(server.addr, &server.read_token).await;
    let mut sorted = tools.clone();
    sorted.sort();
    // Sprint 025 (#633): `register_author` left this list. Minting an
    // identity is a Write operation — a read-only token could otherwise
    // manufacture authors, which was half of the delete backdoor.
    assert_eq!(
        sorted,
        vec![
            "event_search".to_string(),
            "memory_related".to_string(),
            "memory_search".to_string(),
        ],
        "read-only surface drift: {tools:?}"
    );
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn tools_list_write_sees_read_and_write_no_admin() {
    let server = TestServer::spawn().await;
    let tools = list_tools_with(server.addr, &server.write_token).await;
    let mut sorted = tools.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec![
            "dissent_propose".to_string(),
            "event_search".to_string(),
            "memory_add".to_string(),
            "memory_append_event".to_string(),
            "memory_delete".to_string(),
            "memory_related".to_string(),
            "memory_search".to_string(),
            // Sprint 029 (#638) — lifecycle verbs sit at Write.
            "memory_supersede".to_string(),
            "memory_update".to_string(),
            "register_author".to_string(),
        ],
        "write surface drift: {tools:?}"
    );
}
