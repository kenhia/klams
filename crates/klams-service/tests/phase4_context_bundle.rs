//! Phase 4 — T016: integration check that `POST /memory/context`
//! returns all three sections from the seeded fixture and respects
//! the requested token budget.
//!
//! Marked `#[ignore]` so it only runs under the live test stack
//! (`cargo test --workspace -- --ignored`).
//!
//! Sprint 031 (#687): spawns **isolated**. Both tests make claims
//! about what a *budgeted* bundle contains, and a budget is a ranking
//! decision — under 4k tokens the events section only survives if the
//! facts and knowledge competing for the budget are this test's own.
//! On the shared corpus they were whatever else happened to be
//! running, so `events section empty` was a coin flip at default
//! parallelism.

mod common;

use common::{fixture, seed, TestServer};
use klams_types::{ContextRequest, RetrievalFilters};

#[tokio::test]
#[ignore = "requires live test stack (tests/docker-compose.test.yml)"]
async fn context_bundle_returns_all_three_sections() {
    let server = TestServer::spawn_isolated().await;
    let fx = fixture::generate(fixture::FixtureScale::tiny());
    let report = seed::load(&server.store, &fx).await;
    assert!(report.facts > 0 && report.knowledge > 0 && report.events > 0);

    let req = ContextRequest {
        query: fixture::MARKER_TERM.to_string(),
        token_budget: 4_000,
        filters: None,
    };
    let bundle = server
        .client
        .memory_context(&req)
        .await
        .expect("memory_context");

    assert!(!bundle.facts.is_empty(), "facts section empty: {bundle:?}");
    assert!(
        !bundle.knowledge.is_empty(),
        "knowledge section empty: {bundle:?}"
    );
    assert!(
        !bundle.events.is_empty(),
        "events section empty: {bundle:?}"
    );
    assert!(
        bundle.total_spent <= req.token_budget,
        "total_spent {} > budget {}",
        bundle.total_spent,
        req.token_budget
    );

    // Section meta should account for every populated section.
    for key in ["facts", "knowledge", "events"] {
        let meta = bundle
            .sections
            .get(key)
            .unwrap_or_else(|| panic!("missing section meta for {key}: {:?}", bundle.sections));
        assert!(meta.count > 0, "section {key} reports zero count: {meta:?}");
    }

    server.cleanup().await;
}

#[tokio::test]
#[ignore = "requires live test stack"]
async fn context_bundle_respects_tight_budget() {
    let server = TestServer::spawn_isolated().await;
    let fx = fixture::generate(fixture::FixtureScale::tiny());
    let _ = seed::load(&server.store, &fx).await;

    let req = ContextRequest {
        query: fixture::MARKER_TERM.to_string(),
        token_budget: 200,
        filters: Some(RetrievalFilters::default()),
    };
    let bundle = server.client.memory_context(&req).await.expect("context");
    assert!(
        bundle.total_spent <= req.token_budget,
        "spent {} exceeded tight budget {}",
        bundle.total_spent,
        req.token_budget
    );

    server.cleanup().await;
}
