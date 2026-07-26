//! Sprint 029 (#638) — the knowledge lifecycle verbs, end to end:
//! `memory_supersede`, `memory_update`, and `memory_add`'s
//! similar-on-write nudge, against the real MCP HTTP transport (the
//! ownership checks read caller identity from `require_bearer`
//! extensions, so direct `run` calls would prove nothing about the
//! path an agent takes).
//!
//! Run via
//!   `cargo test -p klams-service --test mcp_lifecycle_verbs -- --ignored --test-threads=1`
//! after the docker compose test stack is up.

mod common;

use common::{McpSession, TestServer};
use uuid::Uuid;

/// Write a knowledge memory through `session`, return its id.
async fn seed_knowledge(session: &McpSession, text: &str) -> String {
    let out = session
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "knowledge",
                "text": text,
                "tags": ["s029", "lifecycle"],
            }),
        )
        .await;
    out["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no id in memory_add output: {out}"))
        .to_string()
}

fn same_uuid(a: &str, b: &str) -> bool {
    match (Uuid::parse_str(a), Uuid::parse_str(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// The acceptance case from the WI: one call passing only
/// `(old_id, new_text)` replaces a stale memory; the old record stops
/// surfacing in `memory_search`; the link is inspectable.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn supersede_replaces_hides_and_links() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;

    let old_id = seed_knowledge(
        &owner,
        "s029 zebra-quokka fact: rpidash3 is NOT yet on tailscale",
    )
    .await;
    let out = owner
        .call_tool(
            "memory_supersede",
            serde_json::json!({
                "id": old_id,
                "text": "s029 zebra-quokka fact: rpidash3 joined tailscale on 2026-07-26",
            }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&out),
        None,
        "supersede of own memory must succeed: {out}"
    );
    let new_id = out["id"].as_str().expect("new memory id").to_string();
    assert!(
        same_uuid(out["supersedes"].as_str().unwrap_or_default(), &old_id),
        "replacement must carry the supersedes pointer: {out}"
    );
    // Tags inherit when omitted.
    assert!(
        out["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|v| v.as_str() == Some("lifecycle"))),
        "tags must inherit from the superseded memory: {out}"
    );

    // The old record no longer surfaces; the replacement does.
    let hits = owner
        .call_tool(
            "memory_search",
            serde_json::json!({
                "query": "s029 zebra-quokka rpidash3 tailscale",
                "kinds": ["knowledge"],
                "top_k": 10,
            }),
        )
        .await;
    let ids: Vec<String> = hits
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["memory"]["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        !ids.iter().any(|i| same_uuid(i, &old_id)),
        "superseded memory must be hidden from search: {ids:?}"
    );
    assert!(
        ids.iter().any(|i| same_uuid(i, &new_id)),
        "the replacement must be retrievable: {ids:?}"
    );

    // The link is inspectable on the admin surface: the old record is
    // listed among deleted with `superseded_by` pointing forward.
    let admin = McpSession::handshake(server.addr, &server.bearer_token).await;
    let deleted = admin
        .call_tool(
            "memory_admin_list_deleted",
            serde_json::json!({ "kinds": ["knowledge"] }),
        )
        .await;
    let row = deleted["results"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|d| d["id"].as_str().is_some_and(|i| same_uuid(i, &old_id)))
        .unwrap_or_else(|| panic!("superseded memory must appear in list_deleted: {deleted}"));
    assert!(
        row["superseded_by"]
            .as_str()
            .is_some_and(|s| same_uuid(s, &new_id)),
        "the old record must name its replacement: {row}"
    );

    server.cleanup().await;
}

/// Superseding somebody else's memory is `manage`-tier work.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn cross_author_supersede_requires_manage() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;
    let intruder = McpSession::handshake(server.addr, &server.other_write_token).await;
    let curator = McpSession::handshake(server.addr, &server.manage_token).await;

    let id = seed_knowledge(&owner, "s029 cross-author supersede target").await;

    let refused = intruder
        .call_tool(
            "memory_supersede",
            serde_json::json!({ "id": id, "text": "hijacked" }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&refused),
        Some("INSUFFICIENT_SCOPE"),
        "write-scoped cross-author supersede must be refused: {refused}"
    );

    let allowed = curator
        .call_tool(
            "memory_supersede",
            serde_json::json!({ "id": id, "text": "s029 curated replacement by manage tier" }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&allowed),
        None,
        "manage tier must be able to supersede across authors: {allowed}"
    );

    server.cleanup().await;
}

/// A second supersede of the same (now hidden) record is refused with
/// a pointer forward instead of silently forking the lineage.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn superseding_an_already_superseded_memory_is_refused() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;

    let id = seed_knowledge(&owner, "s029 double-supersede target").await;
    let first = owner
        .call_tool(
            "memory_supersede",
            serde_json::json!({ "id": id, "text": "s029 first replacement" }),
        )
        .await;
    assert_eq!(McpSession::error_code(&first), None, "{first}");

    let second = owner
        .call_tool(
            "memory_supersede",
            serde_json::json!({ "id": id, "text": "s029 second replacement" }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&second),
        Some("NOT_FOUND"),
        "superseding a superseded record must be refused: {second}"
    );

    server.cleanup().await;
}

/// `memory_update` edits in place: id stable, text re-embedded, tags
/// replaceable — and the tags-only path keeps the stored vector.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn update_edits_in_place_with_a_stable_id() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;

    let id = seed_knowledge(&owner, "s029 update target with a typo: kbus0").await;

    let out = owner
        .call_tool(
            "memory_update",
            serde_json::json!({ "id": id, "text": "s029 update target fixed: kubs0" }),
        )
        .await;
    assert_eq!(McpSession::error_code(&out), None, "{out}");
    assert!(
        same_uuid(out["id"].as_str().unwrap_or_default(), &id),
        "update must keep the id stable: {out}"
    );
    assert_eq!(
        out["text"].as_str(),
        Some("s029 update target fixed: kubs0")
    );

    // Tags-only edit (no re-embed): text stays, tags change.
    let out = owner
        .call_tool(
            "memory_update",
            serde_json::json!({ "id": id, "tags": ["s029", "retagged"] }),
        )
        .await;
    assert_eq!(McpSession::error_code(&out), None, "{out}");
    assert_eq!(
        out["text"].as_str(),
        Some("s029 update target fixed: kubs0")
    );
    assert!(
        out["tags"]
            .as_array()
            .is_some_and(|t| t.iter().any(|v| v.as_str() == Some("retagged"))),
        "tags must be replaced: {out}"
    );

    // The updated text is what search now returns for this memory.
    let hits = owner
        .call_tool(
            "memory_search",
            serde_json::json!({ "query": "s029 update target kubs0", "kinds": ["knowledge"] }),
        )
        .await;
    let found = hits.as_array().into_iter().flatten().find(|h| {
        h["memory"]["id"]
            .as_str()
            .is_some_and(|i| same_uuid(i, &id))
    });
    assert!(
        found.is_some_and(|h| h["memory"]["text"]
            .as_str()
            .is_some_and(|t| t.contains("fixed"))),
        "search must serve the updated text: {hits}"
    );

    server.cleanup().await;
}

/// Cross-author update is `manage`-tier work, and a nothing-to-update
/// call fails loudly instead of no-opping.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn update_authorization_and_empty_change_validation() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;
    let intruder = McpSession::handshake(server.addr, &server.other_write_token).await;

    let id = seed_knowledge(&owner, "s029 update authz target").await;

    let refused = intruder
        .call_tool(
            "memory_update",
            serde_json::json!({ "id": id, "text": "vandalized" }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&refused),
        Some("INSUFFICIENT_SCOPE"),
        "cross-author update must be refused: {refused}"
    );

    let empty = owner
        .call_tool("memory_update", serde_json::json!({ "id": id }))
        .await;
    assert_eq!(
        McpSession::error_code(&empty),
        Some("SCHEMA_VALIDATION_FAILED"),
        "an update that changes nothing must be refused: {empty}"
    );

    server.cleanup().await;
}

/// Scanner-ingested knowledge is derived data: the lifecycle verbs
/// refuse it and point at the file instead.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn scanner_chunks_cannot_be_superseded_or_updated() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;

    // Seed a Task-source point directly against the store, as the
    // scanner does.
    let chunk_id = Uuid::now_v7();
    let text = "s029 scanner chunk: build with cargo build --release";
    let embedding = server
        .store
        .embedder
        .embed(text)
        .await
        .expect("test TEI embed");
    server
        .store
        .qdrant
        .index_knowledge(
            klams_types::IndexKnowledge {
                id: chunk_id,
                text: text.to_string(),
                content_hash: format!("s029-scanner-{chunk_id}"),
                source: klams_types::Source::Task,
                tags: vec![],
                repo: Some("klams".into()),
                file: Some("/home/ken/src/klams/README.md".into()),
                machine: Some("kubs0".into()),
                author_id: klams_types::SYSTEM_AUTHOR_ID,
                chunk_index: None,
                language: None,
                heading_path: None,
                symbols: vec![],
                volatility: None,
                supersedes: None,
            },
            embedding,
        )
        .await
        .expect("seed scanner chunk");

    for (tool, args) in [
        (
            "memory_supersede",
            serde_json::json!({ "id": chunk_id.to_string(), "text": "nope" }),
        ),
        (
            "memory_update",
            serde_json::json!({ "id": chunk_id.to_string(), "text": "nope" }),
        ),
    ] {
        let out = owner.call_tool(tool, args).await;
        assert_eq!(
            McpSession::error_code(&out),
            Some("NOT_AGENT_AUTHORED"),
            "{tool} must refuse scanner chunks: {out}"
        );
    }

    server.cleanup().await;
}

/// The similar-on-write nudge: writing near-identical text returns the
/// existing memory so the writer supersedes instead of duplicating.
#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn memory_add_nudges_on_a_near_duplicate() {
    let server = TestServer::spawn_isolated().await;
    let owner = McpSession::handshake(server.addr, &server.author_token).await;

    let text = "s029 similar-on-write: the klams backup path is /gratch/klams-backup";
    let first_id = seed_knowledge(&owner, text).await;

    // Identical text → cosine ~1.0, comfortably above any threshold.
    let out = owner
        .call_tool(
            "memory_add",
            serde_json::json!({ "kind": "knowledge", "text": text }),
        )
        .await;
    assert_eq!(McpSession::error_code(&out), None, "{out}");
    let similar = out["similar_existing"]
        .as_array()
        .unwrap_or_else(|| panic!("twin write must carry similar_existing: {out}"));
    assert!(
        similar
            .iter()
            .any(|s| s["id"].as_str().is_some_and(|i| same_uuid(i, &first_id))),
        "the nudge must name the existing near-duplicate: {out}"
    );

    // A clearly-different write carries no nudge.
    let out = owner
        .call_tool(
            "memory_add",
            serde_json::json!({
                "kind": "knowledge",
                "text": "s029 unrelated: grafana dashboards live in deploy/grafana",
            }),
        )
        .await;
    assert_eq!(McpSession::error_code(&out), None, "{out}");
    assert!(
        out.get("similar_existing").is_none()
            || out["similar_existing"]
                .as_array()
                .is_some_and(Vec::is_empty),
        "an unrelated write must not be nudged: {out}"
    );

    server.cleanup().await;
}
