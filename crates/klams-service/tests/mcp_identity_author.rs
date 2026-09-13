//! Sprint 018 (WI #62) — write tools fall back to the caller's author.
//!
//! An authenticated caller declaring an `agent_name` can call
//! `memory_add` without `author_id` and have the write attributed to
//! that identity's author — no prior `register_author` needed. An
//! explicit `author_id` always wins, so other identities remain
//! reachable (`register_author` flow unchanged).
//!
//! Sprint 052: the binding used to come from a bearer token; it now
//! comes from the declared name. The fallback behaviour is unchanged,
//! which is why this suite survived the deletion.
//!
//! Marked `#[ignore]` like the rest of the integration suite — run via
//!   `cargo test -p klams-service --test mcp_identity_author -- --ignored`
//! after `docker compose -f tests/docker-compose.test.yml up -d`.

mod common;

use common::{McpSession, TestServer};

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn memory_add_without_author_id_attributes_to_the_callers_author() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_agent).await;
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
        "write must be attributed to the caller's bound author: {out}"
    );
}

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn explicit_author_id_still_wins_over_the_callers_binding() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_agent).await;
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
async fn append_event_without_author_id_attributes_to_the_callers_author() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_agent).await;
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
        "event must be attributed to the caller's bound author: {out}"
    );
}
