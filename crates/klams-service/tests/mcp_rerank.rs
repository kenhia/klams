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
//! Runs under `just test-integration` (sprint 036, #731): the test
//! stack now includes a CPU reranker service on 127.0.0.1:57071 and the
//! recipe wires `TEST_RERANKER_URL` to it — the live halves self-skip
//! only when that env var is absent (e.g. a hand-run without the
//! recipe).
//!
//! Sprint 036 (#730): REST `/memory/search` runs the same core
//! pipeline, so the set-preservation contract is asserted on both
//! surfaces here.

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

#[tokio::test]
#[ignore = "requires docker compose test stack"]
async fn rest_search_runs_the_same_reranked_pipeline() {
    // Sprint 036 (#730): the REST surface calls the same core pipeline,
    // reranker included. Same set-preservation contract as the MCP
    // half, plus the additive wire fields the unification brought.
    let Ok(url) = std::env::var("TEST_RERANKER_URL") else {
        eprintln!(
            "skipping rest_search_runs_the_same_reranked_pipeline: TEST_RERANKER_URL not set"
        );
        return;
    };
    let server = TestServer::spawn_isolated_with_reranker(&url).await;
    let session = McpSession::handshake(server.addr, &server.author_token).await;

    let mut seeded = Vec::new();
    for text in SEEDS {
        seeded.push(session.seed_knowledge(text, &["s030", "rerank"]).await);
    }
    let results = server
        .client
        .search(&klams_types::SearchRequest {
            query: "s030 xylophone-badger reranker".into(),
            types: Some(vec![klams_types::SearchType::Knowledge]),
            filters: None,
            top_k: 10,
        })
        .await
        .expect("REST search must not fail with a live reranker");
    assert!(!results.degraded, "nothing should be degraded: {results:?}");
    let ids: Vec<String> = results.results.iter().map(|h| h.id.to_string()).collect();
    for want in &seeded {
        assert_eq!(
            ids.iter().filter(|id| *id == want).count(),
            1,
            "every seeded memory must appear exactly once over REST (got {ids:?})"
        );
    }
    for hit in &results.results {
        assert!(
            hit.raw_score.is_some(),
            "unified pipeline hits carry raw_score"
        );
        assert!(
            hit.source_rank.is_some(),
            "unified pipeline hits carry source_rank"
        );
    }
    server.cleanup().await;
}
