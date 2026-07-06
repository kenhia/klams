//! Phase 4 — T033: summarization pipeline integration tests.
//!
//! Three #[ignore] tests against the live docker-compose test stack:
//!  (a) end-to-end extractive pipeline (`llm_fallback = false`,
//!      bypassing the Ollama probe entirely) — runs one cycle of
//!      `SummarizationTask`, asserts a `summaries` row landed and
//!      that `/memory/context` substitutes the events section when
//!      the raw events would blow the budget.
//!  (b) summarization disabled — `/memory/context` still returns a
//!      non-empty bundle from raw items only (SC-007).
//!  (c) invalidation fallback — after a summary is written and
//!      `invalidate_event_summaries` is called, the next
//!      `/memory/context` call returns raw events again.

#![allow(clippy::too_many_lines)]

mod common;

use common::{fixture, seed, TestServer};
use klams_core::summarize::{StoreEventSource, SummarizationConfig, SummarizationTask};
use klams_store::SummaryStore;
use klams_types::{ContextRequest, SectionSource};
use std::sync::Arc;
use std::time::Duration;

fn extractive_cfg() -> SummarizationConfig {
    SummarizationConfig {
        enabled: true,
        event_cluster_min: 2,
        llm_fallback: false,
        task_interval: Duration::from_secs(3600),
        llm_url: "http://127.0.0.1:1/v1".into(), // never contacted with llm_fallback=false
        llm_model: "unused".into(),
        llm_api_key: None,
    }
}

fn dense_scale() -> fixture::FixtureScale {
    // 5 hosts × 5 categories × 3 days = 75 cluster-days; 500 events
    // → ~6.7 per cluster → safely above event_cluster_min=2.
    fixture::FixtureScale {
        facts: 50,
        knowledge: 50,
        events: 500,
        event_days: 3,
    }
}

#[tokio::test]
#[ignore = "requires live test stack"]
async fn extractive_pipeline_writes_summaries_and_substitutes_in_bundle() {
    seed::truncate_pg().await;
    let server = TestServer::spawn_with_summary_store(true).await;
    let fx = fixture::generate(dense_scale());
    let report = seed::load(&server.store, &fx).await;
    eprintln!("seeded: {report:?}");

    let task = SummarizationTask::new(
        extractive_cfg(),
        Arc::new(StoreEventSource::new(Arc::clone(&server.store))),
        Arc::clone(&server.store) as Arc<dyn SummaryStore>,
    );
    let written = task.run_cycle().await.expect("summarization cycle");
    eprintln!("summaries written: {written}");
    assert!(
        written > 0,
        "summarization should write at least one summary"
    );

    // Force substitution: tiny token budget so raw events tokens
    // dominate and `ContextBuilder` swaps in summaries.
    let bundle = server
        .client
        .memory_context(&ContextRequest {
            query: fixture::MARKER_TERM.to_string(),
            token_budget: 400,
            filters: None,
        })
        .await
        .expect("memory_context");
    let events_meta = bundle.sections.get("events").expect("events section meta");
    eprintln!("events section: {events_meta:?}");
    assert_eq!(
        events_meta.source,
        SectionSource::Summary,
        "events section should be backed by summaries under tight budget"
    );
    assert!(
        !bundle.events.is_empty(),
        "events section should still contain items (now summaries)"
    );
}

#[tokio::test]
#[ignore = "requires live test stack"]
async fn summarization_disabled_returns_raw_bundle_within_budget() {
    // `spawn()` does not wire a SummaryStore into ContextBuilder —
    // equivalent to `[summarization] enabled = false` from the
    // retrieval path's perspective.
    seed::truncate_pg().await;
    let server = TestServer::spawn().await;
    let fx = fixture::generate(dense_scale());
    let report = seed::load(&server.store, &fx).await;
    eprintln!("seeded: {report:?}");

    let bundle = server
        .client
        .memory_context(&ContextRequest {
            query: fixture::MARKER_TERM.to_string(),
            token_budget: 2000,
            filters: None,
        })
        .await
        .expect("memory_context");
    let events_meta = bundle.sections.get("events").expect("events section meta");
    assert_eq!(
        events_meta.source,
        SectionSource::Raw,
        "without a SummaryStore the events section must stay raw"
    );
    assert!(
        !bundle.events.is_empty(),
        "raw events section must be non-empty"
    );
    assert!(
        bundle.total_spent <= 2000,
        "budget must be respected: spent {} > 2000",
        bundle.total_spent
    );
}

#[tokio::test]
#[ignore = "requires live test stack"]
async fn invalidated_summaries_fall_back_to_raw_events() {
    seed::truncate_pg().await;
    let server = TestServer::spawn_with_summary_store(true).await;
    let fx = fixture::generate(dense_scale());
    let report = seed::load(&server.store, &fx).await;
    eprintln!("seeded: {report:?}");

    // Run a cycle so summaries exist.
    let task = SummarizationTask::new(
        extractive_cfg(),
        Arc::new(StoreEventSource::new(Arc::clone(&server.store))),
        Arc::clone(&server.store) as Arc<dyn SummaryStore>,
    );
    let written = task.run_cycle().await.expect("summarization cycle");
    assert!(written > 0);

    // Sanity check that substitution is active before invalidation.
    let req = ContextRequest {
        query: fixture::MARKER_TERM.to_string(),
        token_budget: 400,
        filters: None,
    };
    let pre = server
        .client
        .memory_context(&req)
        .await
        .expect("pre bundle");
    assert_eq!(
        pre.sections.get("events").map(|s| s.source),
        Some(SectionSource::Summary),
        "pre-condition: events should be summary-backed"
    );

    // Invalidate every (host, category, day_bucket) cluster directly
    // via Postgres so the next retrieval has no active summaries.
    let pg_url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into());
    let pool = sqlx::PgPool::connect(&pg_url).await.expect("pg connect");
    let rows_affected =
        sqlx::query("UPDATE summaries SET invalidated_at = now() WHERE invalidated_at IS NULL")
            .execute(&pool)
            .await
            .expect("invalidate")
            .rows_affected();
    eprintln!("invalidated {rows_affected} summaries");
    assert!(rows_affected > 0);

    let post = server
        .client
        .memory_context(&req)
        .await
        .expect("post bundle");
    let events_meta = post.sections.get("events").expect("events section meta");
    eprintln!("post-invalidation events section: {events_meta:?}");
    assert_eq!(
        events_meta.source,
        SectionSource::Raw,
        "after invalidation events section should fall back to raw"
    );
}
