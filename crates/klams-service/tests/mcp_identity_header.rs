//! Sprint 049 (korg WI 2388/2389) — a declared identity authenticates
//! and attributes, end to end.
//!
//! The unit tests in `klams_api::auth` prove the middleware in
//! isolation. This suite proves the part that isolation cannot: that a
//! caller presenting only `X-Homelab-Agent` reaches the real MCP
//! transport, resolves to a real `authors` row in Postgres, and has its
//! write land under that author. That is the acceptance criterion —
//! "a write with a valid header lands under the right author" — and it
//! spans the middleware, the store, and the author-resolution path that
//! was already keyed on `agent_name` before this sprint.
//!
//! Sprint 052: it also pins the deletion — no bearer authenticates,
//! and one that is presented gets a self-diagnosing 401.
//!
//! Marked `#[ignore]` like the rest of the integration suite — run via
//!   `cargo test -p klams-service --test mcp_identity_header -- --ignored`
//! after `docker compose -f tests/docker-compose.test.yml up -d`.

mod common;

use common::{Credential, McpSession, TestServer, INIT_BODY};

fn identity(server: &TestServer) -> Credential {
    Credential::Identity(server.identity_agent_name.clone())
}

/// The headline: no secret presented, and the write is attributed to
/// the declared identity's author.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn a_declared_identity_write_lands_under_its_own_author() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake_with(server.addr, identity(&server)).await;
    let out = session
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI2388_IDENTITY_HEADER", "value": "ok"},
            }),
        )
        .await;
    assert_eq!(
        out["author"]["agent_name"].as_str(),
        Some(server.identity_agent_name.as_str()),
        "a header-authenticated write must attribute to the declared identity: {out}"
    );
    assert_eq!(
        out["author"]["id"].as_str(),
        Some(server.identity_author_id.to_string().as_str()),
        "and to the author row the identity resolved to at startup: {out}"
    );
}

/// Two declared identities against one running service attribute to
/// their own authors and do not collapse into one.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn two_identities_attribute_to_their_own_authors() {
    let server = TestServer::spawn().await;

    let by_header = McpSession::handshake_with(server.addr, identity(&server)).await;
    let header_out = by_header
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI2388_WINDOW_HEADER", "value": "ok"},
            }),
        )
        .await;

    let by_name = McpSession::handshake(server.addr, &server.author_agent).await;
    let other_out = by_name
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI2388_WINDOW_OTHER", "value": "ok"},
            }),
        )
        .await;

    assert_eq!(
        header_out["author"]["agent_name"].as_str(),
        Some(server.identity_agent_name.as_str()),
        "{header_out}"
    );
    assert_eq!(
        other_out["author"]["agent_name"].as_str(),
        Some(server.author_agent_name.as_str()),
        "{other_out}"
    );
    assert_ne!(
        header_out["author"]["id"].as_str(),
        other_out["author"]["id"].as_str(),
        "the two identities name different agents and must not collapse"
    );
}

/// An unknown declared name is a 401 on the MCP mount.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn an_unknown_declared_name_is_401_on_the_mcp_mount() {
    let server = TestServer::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Homelab-Agent", "not-a-configured-identity")
        .body(INIT_BODY)
        .send()
        .await
        .expect("POST /mcp");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// A caller that declares an unknown name while also sending a bearer
/// is refused. Nothing falls through — there is nothing left to fall
/// through to (sprint 052).
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn a_bad_declared_name_is_refused_even_with_a_bearer() {
    let server = TestServer::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("X-Homelab-Agent", "not-a-configured-identity")
        .header("Authorization", "Bearer anything-at-all")
        .body(INIT_BODY)
        .send()
        .await
        .expect("POST /mcp");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// Sprint 052, end to end on the real transport: a bearer and no name
/// is a 401 whose body names the header to send and the one to drop.
/// The unit test pins the middleware; this pins that the body survives
/// the MCP mount rather than being flattened into a bare status.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn a_retired_bearer_gets_a_self_diagnosing_401_on_the_mcp_mount() {
    let server = TestServer::spawn().await;
    let resp = reqwest::Client::new()
        .post(format!("http://{}/mcp", server.addr))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", "Bearer a-retired-credential")
        .body(INIT_BODY)
        .send()
        .await
        .expect("POST /mcp");
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body = resp.text().await.expect("body");
    assert!(body.contains("bearer_retired"), "{body}");
    assert!(body.contains("X-Homelab-Agent"), "{body}");
}

/// The tool catalog is filtered by the identity's scopes — the declared
/// name carries the scope set, so nothing about catalog filtering is
/// special-cased.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn the_catalog_is_filtered_by_the_identitys_scopes() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake_with(server.addr, identity(&server)).await;
    let tools = session.list_tool_names().await;
    assert!(tools.contains(&"memory_search".to_string()), "{tools:?}");
    assert!(tools.contains(&"memory_add".to_string()), "{tools:?}");
    assert!(
        !tools.iter().any(|t| t.starts_with("memory_admin_")),
        "a read+write identity must not see admin tools: {tools:?}"
    );
}

/// REST as well as MCP: the layer is shared, and this proves the header
/// reaches both mounts rather than only the one that was tested.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn the_header_authenticates_the_rest_surface_too() {
    let server = TestServer::spawn().await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/memory/policy", server.addr))
        .header("X-Homelab-Agent", &server.identity_agent_name)
        .send()
        .await
        .expect("GET /memory/policy");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let anon = reqwest::Client::new()
        .get(format!("http://{}/memory/policy", server.addr))
        .send()
        .await
        .expect("GET /memory/policy");
    assert_eq!(anon.status(), reqwest::StatusCode::UNAUTHORIZED);
}
