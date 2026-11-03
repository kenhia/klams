//! Sprint 018 (WI #62) — write tools fall back to the caller's author.
//!
//! An authenticated caller declaring an `agent_name` can call
//! `memory_add` without `author_id` and have the write attributed to
//! that identity's author — no prior `register_author` needed.
//!
//! Sprint 057 (WI 4323): an explicit `author_id` used to win, which let
//! any `write` identity publish as another agent. It may now only
//! confirm the caller's own author; a foreign one is refused on all
//! three create tools.
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
async fn foreign_author_id_is_refused_on_every_create_tool() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_agent).await;
    let foreign = server.other_author_id.to_string();
    let calls = [
        (
            "memory_add",
            serde_json::json!({
                "author_id": foreign,
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI4323_FOREIGN_AUTHOR", "value": "no"},
            }),
        ),
        (
            "memory_append_event",
            serde_json::json!({
                "author_id": foreign,
                "category": "test",
                "payload": {"kind": "wi4323-foreign"},
            }),
        ),
        (
            "dissent_propose",
            serde_json::json!({
                "author_id": foreign,
                "fact_id": uuid::Uuid::now_v7().to_string(),
                "proposed_payload": {"k": "v"},
                "reason": "wi4323",
            }),
        ),
    ];
    for (tool, args) in calls {
        let out = session.call_tool(tool, args).await;
        assert_eq!(
            McpSession::error_code(&out),
            Some("INSUFFICIENT_SCOPE"),
            "{tool} must refuse a foreign author_id: {out}"
        );
    }
}

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn own_author_id_is_accepted() {
    let server = TestServer::spawn().await;
    let session = McpSession::handshake(server.addr, &server.author_agent).await;
    let out = session
        .call_tool(
            "memory_add",
            serde_json::json!({
                "author_id": server.bound_author_id.to_string(),
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": "WI4323_OWN_AUTHOR", "value": "ok"},
            }),
        )
        .await;
    assert_eq!(
        out["author"]["agent_name"].as_str(),
        Some(server.author_agent_name.as_str()),
        "own author_id must be accepted: {out}"
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
