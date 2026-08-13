//! Sprint 043 (WI #1216) — MCP revision 2026-07-28 wire shape.
//!
//! These tests exist because the failure they guard is *silent*. A server
//! that advertises 2026-07-28 without emitting SEP-2549 cache metadata on
//! `tools/list` makes every Claude Code >= 2.1.227 client validate the
//! response, fail, and register ZERO TOOLS — while still showing as
//! connected with its instructions delivered (korg:1212). Nothing in the
//! server logs says so.
//!
//! They are raw JSON-RPC on purpose: rmcp 3.1.2's own *client* cannot
//! drive a conformant 2026-07-28 session (`ClientInfo::default()` sends
//! `ProtocolVersion::LATEST`, still 2025-11-25, and sets no per-request
//! `_meta`), so a client-driven test could not reach this shape at all.
//!
//! A conformant non-initialize request under this revision needs all of:
//!   - `_meta` carrying `io.modelcontextprotocol/protocolVersion` and
//!     `io.modelcontextprotocol/clientCapabilities`,
//!   - the `MCP-Protocol-Version` header *agreeing* with that value
//!     (rmcp rejects a mismatch),
//!   - the SEP-2243 `Mcp-Method` header naming the body's method
//!     (`Mcp-Name` too, for `tools/call`).
//!
//! 2026-07-28 uses the INLINE lifecycle: initialize issues no session id,
//! and later requests carry no `mcp-session-id`.
//!
//! NOTE: this suite proves the *wire shape*, which is not the same as
//! proving client acceptance. The real gate is a live Claude Code session
//! enumerating the tools and completing a real call — see the sprint doc.
//!
//! Marked `#[ignore]` like the rest of the integration suite — run via
//!   `cargo test -p klams-service --test mcp_protocol_2026_07_28 -- --ignored`
//! after `docker compose -f tests/docker-compose.test.yml up -d`.

mod common;

use common::{parse_sse_json, TestServer};
use serde_json::{json, Value};

const V_2026: &str = "2026-07-28";
const V_2025: &str = "2025-11-25";

/// `initialize` for a given revision. Returns the parsed JSON-RPC result
/// and the `mcp-session-id`, if the server issued one.
async fn initialize(server: &TestServer, token: &str, version: &str) -> (Value, Option<String>) {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": version,
            "capabilities": {},
            "clientInfo": {"name": "wire-probe", "version": "0"}
        }
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .body(body.to_string())
        .send()
        .await
        .expect("initialize");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::OK,
        "initialize({version}) should be accepted"
    );
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let text = resp.text().await.expect("initialize body");
    (parse_sse_json(&text), session_id)
}

/// Drive `tools/list` under the 2026-07-28 inline lifecycle and return the
/// `result` object.
async fn tools_list_2026(server: &TestServer, token: &str) -> Value {
    let (init, session_id) = initialize(server, token, V_2026).await;
    assert_eq!(
        init["result"]["protocolVersion"], V_2026,
        "server must negotiate 2026-07-28 when asked for it"
    );
    assert!(
        session_id.is_none(),
        "SEP-2567 removes sessions from 2026-07-28; server issued {session_id:?}"
    );

    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": V_2026,
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {token}"))
        .header("MCP-Protocol-Version", V_2026)
        .header("Mcp-Method", "tools/list")
        .body(body.to_string())
        .send()
        .await
        .expect("tools/list");
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "tools/list ok");
    let text = resp.text().await.expect("tools/list body");
    let parsed = parse_sse_json(&text);
    assert!(
        parsed.get("error").is_none(),
        "tools/list returned an error: {parsed}"
    );
    parsed["result"].clone()
}

/// The headline assertion: both SEP-2549 fields present and well-typed.
///
/// Well-typed matters as much as present — the client's validator rejects
/// `"3600000"` exactly as surely as it rejects `undefined`, and a String
/// here would reproduce the zero-tools bug with the field visibly "set".
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn tools_list_carries_well_typed_cache_metadata_at_2026_07_28() {
    let server = TestServer::spawn().await;
    let result = tools_list_2026(&server, &server.bearer_token).await;

    let ttl = &result["ttlMs"];
    assert!(
        ttl.is_number(),
        "ttlMs must be a JSON number, got {ttl:?} — a string reproduces korg:1212 with the field apparently set"
    );
    assert_eq!(
        ttl.as_u64(),
        Some(3_600_000),
        "ttlMs should be the hour klams advertises"
    );

    assert_eq!(
        result["cacheScope"], "private",
        "klams's catalog is scope-filtered per bearer token, so it is NOT publicly cacheable"
    );

    assert!(
        result["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "a full-scope token should still see a non-empty catalog"
    );
}

/// A 2025-11-25 peer is entitled to 2025-11-25's shape. Strict clients may
/// reject unknown fields; rmcp makes the same call for `resultType`.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn cache_metadata_absent_for_legacy_peer() {
    let server = TestServer::spawn().await;
    let (init, session_id) = initialize(&server, &server.bearer_token, V_2025).await;
    assert_eq!(init["result"]["protocolVersion"], V_2025);
    let session_id = session_id.expect("legacy lifecycle issues a session id");

    let client = reqwest::Client::new();
    let base = format!("http://{}/mcp", server.addr);
    let _ = client
        .post(&base)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {}", server.bearer_token))
        .header("mcp-session-id", &session_id)
        .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
        .send()
        .await
        .expect("initialized notification");

    let resp = client
        .post(&base)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {}", server.bearer_token))
        .header("mcp-session-id", &session_id)
        .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#)
        .send()
        .await
        .expect("tools/list");
    let text = resp.text().await.expect("body");
    let result = parse_sse_json(&text)["result"].clone();

    assert!(
        result["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "legacy peer still gets its catalog"
    );
    assert!(
        result.get("ttlMs").is_none_or(Value::is_null),
        "ttlMs must not be emitted to a 2025-11-25 peer, got {:?}",
        result.get("ttlMs")
    );
    assert!(
        result.get("cacheScope").is_none_or(Value::is_null),
        "cacheScope must not be emitted to a 2025-11-25 peer, got {:?}",
        result.get("cacheScope")
    );
}

/// Every revision klams claims must negotiate as itself, and anything
/// newer must fall back to the pinned ceiling rather than being echoed.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn every_supported_revision_negotiates_as_itself() {
    let server = TestServer::spawn().await;
    for version in ["2024-11-05", "2025-03-26", "2025-06-18", V_2025, V_2026] {
        let (init, _) = initialize(&server, &server.bearer_token, version).await;
        assert_eq!(
            init["result"]["protocolVersion"], version,
            "klams claims {version} in supported_protocol_versions, so it must serve it"
        );
    }
}

/// The raw-probe trick: ask for an absurd future revision and read the
/// server's real ceiling out of the response, without trusting its docs.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn unknown_future_version_falls_back_to_the_pinned_ceiling() {
    let server = TestServer::spawn().await;
    let (init, _) = initialize(&server, &server.bearer_token, "9999-12-31").await;
    assert_eq!(
        init["result"]["protocolVersion"], V_2026,
        "an unknown version must fall back to klams's ceiling, never be echoed"
    );
}

/// Why `cacheScope` is `private` here while kaed and korg-mcp both emit
/// `public`: klams filters the catalog by the caller's token scopes, so
/// two principals get two different catalogs from one server. A shared
/// cache keyed on the request alone would serve one to the other.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn catalog_differs_by_token_scope_which_is_why_it_is_private() {
    let server = TestServer::spawn().await;

    let names = |result: &Value| -> Vec<String> {
        result["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_string())
            .collect()
    };

    let full = tools_list_2026(&server, &server.bearer_token).await;
    let read_only = tools_list_2026(&server, &server.read_token).await;

    let full_names = names(&full);
    let read_names = names(&read_only);

    assert!(
        read_names.len() < full_names.len(),
        "a read-only token must see a strictly smaller catalog ({} vs {})",
        read_names.len(),
        full_names.len()
    );
    assert!(
        read_names.iter().all(|n| full_names.contains(n)),
        "the read-only catalog should be a subset of the full one"
    );
    assert!(
        !read_names.iter().any(|n| n.starts_with("memory_admin_")),
        "a read-only token must not see admin tools: {read_names:?}"
    );

    // Both are private — the divergence is not conditional on scope.
    assert_eq!(full["cacheScope"], "private");
    assert_eq!(read_only["cacheScope"], "private");
}

/// One real `tools/call` through the conformant request shape, proving the
/// MRTR envelope round-trips and that the migration to `CallToolResponse`
/// did not change what callers see.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn tools_call_round_trips_under_2026_07_28() {
    let server = TestServer::spawn().await;
    let (_, session_id) = initialize(&server, &server.bearer_token, V_2026).await;
    assert!(session_id.is_none());

    let body = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "memory_search",
            "arguments": {"query": "wire probe", "top_k": 1},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": V_2026,
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {}", server.bearer_token))
        .header("MCP-Protocol-Version", V_2026)
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "memory_search")
        .body(body.to_string())
        .send()
        .await
        .expect("tools/call");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let text = resp.text().await.expect("body");
    let parsed = parse_sse_json(&text);
    assert!(
        parsed.get("error").is_none(),
        "tools/call errored: {parsed}"
    );
    assert!(
        parsed["result"]["content"]
            .as_array()
            .is_some_and(|c| !c.is_empty()),
        "memory_search should return content, got {parsed}"
    );
    assert_ne!(
        parsed["result"]["isError"], true,
        "memory_search should not be an error envelope: {parsed}"
    );
}
