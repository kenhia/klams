//! Sprint 025 (#633) — `memory_delete` honors ownership, end to end.
//!
//! These drive the real MCP HTTP transport, because the ownership check
//! reads caller identity from request extensions that only
//! `require_bearer` populates — calling `memory_delete::run` directly
//! would prove nothing about the path an agent actually takes.
//!
//! The headline case is the one measured on 2026-07-25: a caller minted
//! a fresh author via `register_author`, passed its id to
//! `memory_delete`, and deleted a memory written by a different author,
//! 6 seconds after the identity came into existence.
//!
//! Run via
//!   `cargo test -p klams-service --test mcp_delete_ownership -- --ignored`
//! after the docker compose test stack is up.

mod common;

use common::{McpSession, TestServer};

/// Write a fact through `session` and return its id.
async fn seed_fact(session: &McpSession, key: &str) -> String {
    let out = session
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "fact",
                "fact_type": "EnvFact",
                "payload": {"key": key, "value": "sprint025"},
            }),
        )
        .await;
    out["memory"]["id"]
        .as_str()
        .or_else(|| out["id"].as_str())
        .unwrap_or_else(|| panic!("no id in memory_add output: {out}"))
        .to_string()
}

/// **The sprint's reason for existing.** A write-scoped token must not
/// be able to delete a memory written by another author.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn write_scoped_caller_cannot_delete_another_authors_memory() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;
    let intruder = McpSession::handshake(server.addr, &server.other_write_token).await;

    let id = seed_fact(&owner, "S025_CROSS_AUTHOR").await;
    let out = intruder
        .call_tool("memory_delete", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        Some("INSUFFICIENT_SCOPE"),
        "cross-author delete must be refused: {out}"
    );

    server.cleanup().await;
}

/// The exact 2026-07-25 repro: mint a throwaway identity, pass its id.
/// The `author_id` argument is no longer an identity selector.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn register_author_backdoor_is_closed() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;
    let intruder = McpSession::handshake(server.addr, &server.other_write_token).await;

    let id = seed_fact(&owner, "S025_BACKDOOR").await;

    // Mint a fresh identity, exactly as the original session did.
    let minted = intruder
        .call_tool(
            "register_author",
            serde_json::json!({ "agent_name": "s025-throwaway" }),
        )
        .await;
    let minted_id = minted["author_id"]
        .as_str()
        .unwrap_or_else(|| panic!("register_author failed: {minted}"));

    let out = intruder
        .call_tool(
            "memory_delete",
            serde_json::json!({ "id": id, "author_id": minted_id }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        Some("INSUFFICIENT_SCOPE"),
        "a minted author_id must not change who the delete acts as: {out}"
    );

    server.cleanup().await;
}

/// Self-management is the common case and needs only `write`.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn author_may_delete_its_own_memory_without_author_id() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;

    let id = seed_fact(&owner, "S025_SELF_MANAGE").await;
    let out = owner
        .call_tool("memory_delete", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        None,
        "an author must be able to retract its own record: {out}"
    );
    assert_eq!(out["id"].as_str().map(str::to_lowercase), {
        let mut s = id.replace('-', "");
        s.make_ascii_lowercase();
        Some(s)
    });

    server.cleanup().await;
}

/// The `manage` tier is what a curator needs: remove somebody else's
/// stale record so the next agent isn't misled by it.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn manage_scoped_caller_may_curate_across_authors() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;
    let curator = McpSession::handshake(server.addr, &server.manage_token).await;

    let id = seed_fact(&owner, "S025_CURATE").await;
    let out = curator
        .call_tool("memory_delete", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        None,
        "manage must permit cross-author curation: {out}"
    );

    server.cleanup().await;
}

/// Naming another author is refused even when the caller *could* have
/// done the delete under its own identity — deletes are never performed
/// on behalf of somebody else.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn manage_scoped_caller_still_cannot_impersonate() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;
    let curator = McpSession::handshake(server.addr, &server.manage_token).await;

    let id = seed_fact(&owner, "S025_NO_IMPERSONATION").await;
    let out = curator
        .call_tool(
            "memory_delete",
            serde_json::json!({ "id": id, "author_id": server.bound_author_id.to_string() }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        Some("INSUFFICIENT_SCOPE"),
        "acting as another author must be refused: {out}"
    );

    server.cleanup().await;
}

/// Sprint 025 (#633) — minting identities moved from `read` to `write`.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn read_only_token_cannot_register_authors() {
    let server = TestServer::spawn_isolated().await;
    let viewer = McpSession::handshake(server.addr, &server.read_token).await;

    let names = viewer.list_tool_names().await;
    assert!(
        !names.iter().any(|n| n == "register_author"),
        "a read-only token must not even see register_author: {names:?}"
    );

    let out = viewer
        .call_tool(
            "register_author",
            serde_json::json!({ "agent_name": "s025-should-not-exist" }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        Some("INSUFFICIENT_SCOPE"),
        "read-only token must not mint identities: {out}"
    );

    server.cleanup().await;
}

/// Sprint 025 (#636) — registration is idempotent per `agent_name`
/// instead of minting a row per call.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn register_author_dedupes_on_agent_name() {
    let server = TestServer::spawn_isolated().await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;

    let first = session
        .call_tool(
            "register_author",
            serde_json::json!({ "agent_name": "s025-dedupe" }),
        )
        .await;
    let second = session
        .call_tool(
            "register_author",
            serde_json::json!({ "agent_name": "s025-dedupe" }),
        )
        .await;
    assert_eq!(
        first["author_id"], second["author_id"],
        "a second register_author must return the same row, not mint another"
    );

    server.cleanup().await;
}

/// An `agent_name` no `[[auth.tokens]]` grant could ever bind to is
/// refused, with a usable suggestion in the message.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn register_author_rejects_unbindable_agent_names() {
    let server = TestServer::spawn_isolated().await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;

    let out = session
        .call_tool(
            "register_author",
            serde_json::json!({ "agent_name": "GitHub Copilot" }),
        )
        .await;
    assert_eq!(McpSession::error_code(&out), Some("INVALID_AGENT_NAME"));
    let text = out["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("github-copilot"),
        "error must suggest a valid name: {out}"
    );

    server.cleanup().await;
}
