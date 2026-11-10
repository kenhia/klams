//! Sprint 058 — the propose-supersede path and `memory_get`'s
//! soft-delete contract, end to end over the MCP HTTP transport against
//! real Postgres and Qdrant: the dedupe index, the conditional claim,
//! the `manage` gate in the dispatcher, and the Qdrant lineage payload.
//!
//! Run via `just test-integration` after `just test-stack-up`.

mod common;

use common::{McpSession, TestServer};
use uuid::Uuid;

fn same_uuid(a: &str, b: &str) -> bool {
    match (Uuid::parse_str(a), Uuid::parse_str(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// WI 4331: a soft-deleted knowledge record is `NOT_FOUND` to
/// `memory_get`, the same answer as an unknown id.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn memory_get_hides_soft_deleted_knowledge() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_agent).await;
    let id = owner
        .seed_knowledge("s058 get-after-delete marmot", &["s058"])
        .await;

    let live = owner
        .call_tool("memory_get", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(McpSession::error_code(&live), None, "{live}");

    let del = owner
        .call_tool("memory_delete", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(McpSession::error_code(&del), None, "{del}");

    let gone = owner
        .call_tool("memory_get", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(McpSession::error_code(&gone), Some("NOT_FOUND"), "{gone}");

    server.cleanup().await;
}

/// The whole flow: a write-only identity proposes against another
/// author's records, a re-file dedupes, the proposer cannot accept, a
/// `manage` identity can, and the retired records point at the merge.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn propose_then_accept_retires_into_the_merge() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_agent).await;
    let proposer = McpSession::handshake(server.addr, &server.other_write_agent).await;
    let curator = McpSession::handshake(server.addr, &server.manage_agent).await;

    let keeper = owner
        .seed_knowledge("s058 keeper: kubs0 hosts klams", &["s058"])
        .await;
    let dup = owner
        .seed_knowledge("s058 dup: klams lives on kubs0", &["s058"])
        .await;

    let filed = proposer
        .call_tool(
            "supersede_propose",
            serde_json::json!({
                "keeper_id": keeper,
                "retire_ids": [dup],
                "text": "s058 merged: klams runs on kubs0",
                "reason": "same fact twice",
            }),
        )
        .await;
    assert_eq!(McpSession::error_code(&filed), None, "{filed}");
    assert_eq!(filed["deduped"], false, "{filed}");
    let pid = filed["proposal"]["id"].as_str().unwrap().to_string();

    let again = proposer
        .call_tool(
            "supersede_propose",
            serde_json::json!({
                "keeper_id": keeper,
                "retire_ids": [dup],
                "text": "s058 merged: klams runs on kubs0",
                "reason": "same fact twice, again",
            }),
        )
        .await;
    assert_eq!(again["deduped"], true, "{again}");
    assert!(same_uuid(again["proposal"]["id"].as_str().unwrap(), &pid));
    assert_eq!(again["proposal"]["submission_count"], 2, "{again}");

    let queue = curator
        .call_tool("supersede_proposals_list", serde_json::json!({}))
        .await;
    assert!(
        queue["proposals"]
            .as_array()
            .is_some_and(|ps| ps.iter().any(|p| p["id"].as_str() == Some(pid.as_str()))),
        "{queue}"
    );

    let refused = proposer
        .call_tool(
            "supersede_proposal_resolve",
            serde_json::json!({ "proposal_ids": [pid], "decision": "accept" }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&refused),
        Some("INSUFFICIENT_SCOPE"),
        "{refused}"
    );

    let done = curator
        .call_tool(
            "supersede_proposal_resolve",
            serde_json::json!({ "proposal_ids": [pid], "decision": "accept" }),
        )
        .await;
    assert_eq!(done["results"][0]["outcome"], "accepted", "{done}");
    let merged = done["results"][0]["result_memory_id"]
        .as_str()
        .unwrap()
        .to_string();

    let got = owner
        .call_tool("memory_get", serde_json::json!({ "id": merged }))
        .await;
    assert_eq!(McpSession::error_code(&got), None, "{got}");
    assert!(
        same_uuid(got["supersedes"].as_str().unwrap_or_default(), &keeper),
        "{got}"
    );
    for retired in [&keeper, &dup] {
        let r = owner
            .call_tool("memory_get", serde_json::json!({ "id": retired }))
            .await;
        assert_eq!(McpSession::error_code(&r), Some("NOT_FOUND"), "{r}");
    }

    // A second accept finds nothing pending.
    let twice = curator
        .call_tool(
            "supersede_proposal_resolve",
            serde_json::json!({ "proposal_ids": [pid], "decision": "accept" }),
        )
        .await;
    assert_eq!(twice["results"][0]["outcome"], "not_pending", "{twice}");

    server.cleanup().await;
}

/// Overseer ruling on korg:4401, against real Qdrant: restoring a
/// superseded record clears `superseded_by` along with the soft-delete
/// pair. `memory_admin_restore` and the proposal-accept rollback share
/// `restore_payload`, so this pins the primitive both depend on.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn restoring_a_superseded_record_clears_superseded_by() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_agent).await;
    let admin = McpSession::handshake(server.addr, &server.full_agent).await;

    let old = owner
        .seed_knowledge("s058 restore target: rpidash3 is offline", &["s058"])
        .await;
    let sup = owner
        .call_tool(
            "memory_supersede",
            serde_json::json!({ "id": old, "text": "s058 restore: rpidash3 is back" }),
        )
        .await;
    assert_eq!(McpSession::error_code(&sup), None, "{sup}");
    let new_id = sup["id"].as_str().unwrap().to_string();

    let restored = admin
        .call_tool("memory_admin_restore", serde_json::json!({ "id": old }))
        .await;
    assert_eq!(McpSession::error_code(&restored), None, "{restored}");

    let got = owner
        .call_tool("memory_get", serde_json::json!({ "id": old }))
        .await;
    assert_eq!(
        McpSession::error_code(&got),
        None,
        "restored is live: {got}"
    );
    assert!(
        got.get("superseded_by").is_none(),
        "a live record must not name a replacement: {got}"
    );
    let replacement = owner
        .call_tool("memory_get", serde_json::json!({ "id": new_id }))
        .await;
    assert!(
        same_uuid(replacement["supersedes"].as_str().unwrap_or_default(), &old),
        "lineage stays on the replacement: {replacement}"
    );

    server.cleanup().await;
}
