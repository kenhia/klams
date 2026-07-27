//! Sprint 018 (WI #62) — write tools fall back to the bearer-bound author.
//!
//! An authenticated caller whose token is bound to an `agent_name` can
//! call `memory_add` without `author_id` and have the write attributed
//! to that author — no prior `register_author` needed. An explicit
//! `author_id` always wins, so identities other than the token binding
//! remain reachable (`register_author` flow unchanged).
//!
//! Marked `#[ignore]` like the rest of the integration suite — run via
//!   `cargo test -p klams-service --test mcp_bearer_author -- --ignored`
//! after `docker compose -f tests/docker-compose.test.yml up -d`.

mod common;

use common::TestServer;

const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}"#;

/// Parse a Streamable-HTTP SSE (or bare-JSON) body into its JSON-RPC payload.
fn parse_sse_json(body: &str) -> serde_json::Value {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        return v;
    }
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(rest) {
                return v;
            }
        }
    }
    panic!("no JSON / data: line in body (len={}):\n{body}", body.len());
}

struct McpSession {
    client: reqwest::Client,
    base: String,
    token: String,
    session_id: String,
}

impl McpSession {
    async fn handshake(addr: std::net::SocketAddr, token: &str) -> Self {
        let client = reqwest::Client::new();
        let base = format!("http://{addr}/mcp");
        let init = client
            .post(&base)
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
        let notif = client
            .post(&base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {token}"))
            .header("mcp-session-id", &session_id)
            .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .send()
            .await
            .expect("initialized notify");
        assert!(notif.status().is_success(), "initialized notify ok");
        Self {
            client,
            base,
            token: token.to_string(),
            session_id,
        }
    }

    /// Call a tool; returns the parsed JSON the tool put in
    /// `result.content[0].text`.
    async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        });
        let resp = self
            .client
            .post(&self.base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("mcp-session-id", &self.session_id)
            .body(body.to_string())
            .send()
            .await
            .expect("tools/call");
        assert!(resp.status().is_success(), "tools/call http ok");
        let rpc = parse_sse_json(&resp.text().await.expect("body"));
        let text = rpc["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("unexpected tools/call response: {rpc}"));
        serde_json::from_str(text).expect("tool result JSON")
    }
}

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn memory_add_without_author_id_attributes_to_bearer_author() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;
    let out = session
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI62_BEARER_FALLBACK", "value": "ok"},
            }),
        )
        .await;
    assert_eq!(
        out["author"]["agent_name"].as_str(),
        Some(server.author_agent_name.as_str()),
        "write must be attributed to the bearer-bound author: {out}"
    );
}

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn explicit_author_id_still_wins_over_bearer_binding() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;
    let out = session
        .call_tool(
            "memory_add",
            serde_json::json!({
                "author_id": klams_types::SYSTEM_AUTHOR_ID,
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI62_EXPLICIT_AUTHOR", "value": "ok"},
            }),
        )
        .await;
    assert_eq!(
        out["author"]["agent_name"].as_str(),
        Some("system"),
        "explicit author_id must not be overridden: {out}"
    );
}

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn append_event_without_author_id_attributes_to_bearer_author() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;
    let out = session
        .call_tool(
            "memory_append_event",
            serde_json::json!({
                "category": "test",
                "payload": {"kind": "wi62-event"},
            }),
        )
        .await;
    assert_eq!(
        out["author"]["agent_name"].as_str(),
        Some(server.author_agent_name.as_str()),
        "event must be attributed to the bearer-bound author: {out}"
    );
}
