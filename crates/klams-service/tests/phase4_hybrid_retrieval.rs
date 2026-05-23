//! Phase 4 — T023: integration coverage for hybrid retrieval.
//!
//! All tests are `#[ignore]` so they only run under the live stack
//! (`tests/docker-compose.test.yml` via `cargo test ... -- --ignored`).
//!
//! Scenarios:
//!   1. Literal vs paraphrase — different phrasings of the same intent
//!      return overlapping knowledge IDs (vector path is doing real
//!      work, not echoing FTS only).
//!   2. Filter pre-pruning — `host=<h>` + `since=<2d ago>` constrains
//!      events to that host and that window; non-matching rows are
//!      removed before scoring.
//!   3. One source returns zero — a filter that no fact/event payload
//!      can satisfy still produces a bundle with knowledge populated.

mod common;

use common::{fixture, seed, TestServer};
use klams_types::{ContextRequest, RetrievalFilters};
use std::collections::HashSet;

#[tokio::test]
#[ignore = "requires live test stack (tests/docker-compose.test.yml)"]
async fn literal_and_paraphrase_share_results() {
    let server = TestServer::spawn().await;
    let fx = fixture::generate(fixture::FixtureScale::small());
    let _ = seed::load(&server.store, &fx).await;

    let literal = server
        .client
        .memory_context(&ContextRequest {
            query: "deployment runbook for production rollouts".into(),
            token_budget: 4_000,
            filters: None,
        })
        .await
        .expect("literal context");

    let paraphrase = server
        .client
        .memory_context(&ContextRequest {
            query: "release procedure for promoting builds to prod".into(),
            token_budget: 4_000,
            filters: None,
        })
        .await
        .expect("paraphrase context");

    assert!(
        !literal.knowledge.is_empty(),
        "literal query returned zero knowledge"
    );
    assert!(
        !paraphrase.knowledge.is_empty(),
        "paraphrase query returned zero knowledge"
    );

    let literal_ids: HashSet<_> = literal.knowledge.iter().map(|i| i.id).collect();
    let overlap = paraphrase
        .knowledge
        .iter()
        .filter(|i| literal_ids.contains(&i.id))
        .count();
    assert!(
        overlap > 0,
        "literal and paraphrase queries shared no knowledge ids \
         (vector semantic matching is not contributing); \
         literal_ids={literal_ids:?}, paraphrase ids={:?}",
        paraphrase
            .knowledge
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "requires live test stack"]
async fn filter_pre_pruning_constrains_events() {
    let server = TestServer::spawn().await;
    let fx = fixture::generate(fixture::FixtureScale::small());
    let _ = seed::load(&server.store, &fx).await;

    let host = "alpha";
    let since = time::OffsetDateTime::now_utc() - time::Duration::days(2);

    let req = ContextRequest {
        query: fixture::MARKER_TERM.to_string(),
        token_budget: 4_000,
        filters: Some(RetrievalFilters {
            host: Some(host.into()),
            since: Some(since),
            ..RetrievalFilters::default()
        }),
    };
    let bundle = server.client.memory_context(&req).await.expect("context");

    assert!(
        !bundle.events.is_empty(),
        "expected at least one event after host+since filter"
    );

    for item in &bundle.events {
        let got_host = item
            .payload
            .get("host")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert_eq!(got_host, host, "event host leaked past filter: {item:?}");

        let observed = item
            .payload
            .get("observed_at")
            .and_then(|v| v.as_str())
            .and_then(|s| {
                time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(|| panic!("event missing observed_at: {item:?}"));
        assert!(
            observed >= since,
            "event observed_at {observed} predates since {since}",
        );
    }
}

#[tokio::test]
#[ignore = "requires live test stack"]
async fn zero_row_filter_on_one_source_leaves_knowledge() {
    let server = TestServer::spawn().await;
    let fx = fixture::generate(fixture::FixtureScale::small());
    let _ = seed::load(&server.store, &fx).await;

    // `repo=docs` only exists on knowledge payloads. Facts and events
    // get filtered out entirely; knowledge survives.
    let req = ContextRequest {
        query: fixture::MARKER_TERM.to_string(),
        token_budget: 4_000,
        filters: Some(RetrievalFilters {
            repo: Some("docs".into()),
            ..RetrievalFilters::default()
        }),
    };
    let bundle = server.client.memory_context(&req).await.expect("context");

    assert!(
        bundle.facts.is_empty(),
        "facts unexpectedly survived repo=docs filter: {:?}",
        bundle.facts
    );
    assert!(
        bundle.events.is_empty(),
        "events unexpectedly survived repo=docs filter: {:?}",
        bundle.events
    );
    assert!(
        !bundle.knowledge.is_empty(),
        "knowledge wiped out by repo=docs even though docs chunks exist"
    );
    for item in &bundle.knowledge {
        let repo = item
            .payload
            .get("repo")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert_eq!(repo, "docs", "knowledge repo leaked past filter: {item:?}");
    }
}
