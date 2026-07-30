//! Sprint 030 (#685) — the second-stage rerank contract, end to end
//! over the real MCP HTTP transport:
//!
//! 1. **Best-effort**: with the reranker configured but DEAD, a search
//!    must still serve the un-reranked order — never an error. This is
//!    the contract that makes `reranker_url` safe to leave on.
//! 2. **Set preservation**: with a LIVE reranker, the stage may only
//!    permute the page — every hit present exactly once, no drops, no
//!    duplicates.
//!
//! Run via
//!   `cargo test -p klams-service --test mcp_rerank -- --ignored --test-threads=1`
//! after the docker compose test stack is up. The live half
//! additionally wants `TEST_RERANKER_URL` (e.g. `http://127.0.0.1:7071`)
//! and self-skips without it.

mod common;

use common::{McpSession, TestServer};

async fn search_ids(session: &McpSession, query: &str) -> Vec<String> {
    let hits = session
        .call_tool(
            "memory_search",
            serde_json::json!({
                "query": query,
                "kinds": ["knowledge"],
                "top_k": 10,
            }),
        )
        .await;
    assert_eq!(
        McpSession::error_code(&hits),
        None,
        "memory_search must not fail: {hits}"
    );
    hits.as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| h["memory"]["id"].as_str().map(str::to_string))
        .collect()
}

const SEEDS: [&str; 3] = [
    "s030 xylophone-badger note: the reranker stage is best-effort by contract",
    "s030 xylophone-badger note: kubs0 serves the cross-encoder on port 7071",
    "s030 xylophone-badger note: pancakes require flour, eggs, and milk",
];

#[tokio::test]
#[ignore = "requires docker compose test stack"]
async fn a_dead_reranker_never_fails_a_search() {
    // Port 9 (discard) refuses immediately — the fastest honest "dead".
    let server = TestServer::spawn_isolated_with_reranker("http://127.0.0.1:9").await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;

    let mut seeded = Vec::new();
    for text in SEEDS {
        seeded.push(session.seed_knowledge(text, &["s030", "rerank"]).await);
    }
    let ids = search_ids(&session, "s030 xylophone-badger reranker").await;
    assert!(
        !ids.is_empty(),
        "the un-reranked fallback order must still be served"
    );
    server.cleanup().await;
}

#[tokio::test]
#[ignore = "requires docker compose test stack"]
async fn a_live_rerank_permutes_but_never_drops_or_duplicates() {
    let Ok(url) = std::env::var("TEST_RERANKER_URL") else {
        eprintln!(
            "skipping a_live_rerank_permutes_but_never_drops_or_duplicates: \
             TEST_RERANKER_URL not set"
        );
        return;
    };
    let server = TestServer::spawn_isolated_with_reranker(&url).await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;

    let mut seeded = Vec::new();
    for text in SEEDS {
        seeded.push(session.seed_knowledge(text, &["s030", "rerank"]).await);
    }
    let ids = search_ids(&session, "s030 xylophone-badger reranker").await;
    for want in &seeded {
        assert_eq!(
            ids.iter().filter(|id| *id == want).count(),
            1,
            "every seeded memory must appear exactly once (got {ids:?})"
        );
    }
    server.cleanup().await;
}
