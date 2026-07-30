//! Sprint 006 T029 (US2) — `restore::run_from` end-to-end roundtrip.
//!
//! Seeds the fixture into the test compose stack, runs `run_once`,
//! truncates the target stores (the in-test equivalent of
//! "tear down + bring up a fresh compose stack" from research.md
//! R-008 — the compose volumes survive across tests), then runs
//! `restore::run_from(date, force=true)` and asserts that fact /
//! event counts plus a canonical 10-row sample of facts match.
//!
//! This integration test IS the once-per-sprint restore exercise
//! that satisfies FR-016. `cargo test -p klams-service --test
//! restore_roundtrip -- --ignored`.

mod common;

use common::fixture::{generate_with_seed, FixtureScale};
use common::{seed, TestServer};
use klams_service::backup::restore::run_from;
use klams_service::backup::{run_once, MaintenanceState, OrchestratorDeps};
use klams_types::SameDayStrategy;
use tempfile::tempdir;

const TEST_COLLECTION: &str = "klams_restore_roundtrip_test";

async fn count_facts(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM facts")
        .fetch_one(pool)
        .await
        .expect("count facts")
}

async fn count_events(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM events")
        .fetch_one(pool)
        .await
        .expect("count events")
}

async fn sample_fact_ids(pool: &sqlx::PgPool) -> Vec<uuid::Uuid> {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM facts ORDER BY id LIMIT 10")
        .fetch_all(pool)
        .await
        .expect("sample facts")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker-compose.test.yml; multi-second roundtrip"]
async fn restore_roundtrip_reproduces_counts_and_sample() {
    // Whole-database restore — see `common::whole_database_guard`.
    let _serial = common::whole_database_guard().await;
    // Drop-then-create: a restored-into collection poisons later
    // snapshot runs on a long-lived stack — see
    // `common::ensure_collection`.
    common::ensure_collection(TEST_COLLECTION, true).await;
    let dir = tempdir().unwrap();
    let state = MaintenanceState::new();

    // Build a TestServer just so we have a CompositeStore wired to
    // the test stack for seeding. We don't drive HTTP.
    let server = TestServer::spawn().await;

    let pool = sqlx::PgPool::connect(&common::test_pg_url())
        .await
        .expect("pg connect");
    // Start from a known-empty PG so counts are deterministic.
    sqlx::query("TRUNCATE facts, events, summaries CASCADE")
        .execute(&pool)
        .await
        .expect("truncate");

    // Sprint 006 R-008: seed a fixture (small preset keeps the
    // roundtrip under ~30s while still proving non-trivial volume).
    let fixture = generate_with_seed(FixtureScale::small(), 0xDA70_0006_0029_0029);
    let report = seed::load(&server.store, &fixture).await;
    assert_eq!(report.facts, fixture.facts.len());
    assert_eq!(report.events, fixture.events.len());

    let facts_before = count_facts(&pool).await;
    let events_before = count_events(&pool).await;
    let sample_before = sample_fact_ids(&pool).await;
    assert!(facts_before > 0);
    assert!(events_before > 0);

    let deps = OrchestratorDeps {
        backup_dir: dir.path().to_path_buf(),
        pg_url: common::test_pg_url(),
        pg_bin_dir: common::pg16_bin_dir(),
        qdrant_rest_url: common::test_qdrant_rest_url(),
        qdrant_collection: TEST_COLLECTION.into(),
        daily_count: 14,
        weekly_count: 4,
        same_day_strategy: SameDayStrategy::Suffix,
        drop_remote_qdrant_snapshot: false,
        state: state.clone(),
        status_hook: None,
        status_hook_timeout: std::time::Duration::from_secs(10),
    };
    let run = run_once(&deps).await.expect("backup ran");
    assert_eq!(run.ok, Some(true), "backup run must be ok");

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Simulate "fresh stack": wipe PG. (Qdrant snapshot upload with
    // priority=snapshot replaces collection contents, so we don't
    // need to delete points first.)
    sqlx::query("TRUNCATE facts, events, summaries CASCADE")
        .execute(&pool)
        .await
        .expect("truncate before restore");
    assert_eq!(count_facts(&pool).await, 0);

    // Restore.
    run_from(&deps, &date, true, |_| {})
        .await
        .expect("restore succeeded");

    let facts_after = count_facts(&pool).await;
    let events_after = count_events(&pool).await;
    let sample_after = sample_fact_ids(&pool).await;
    assert_eq!(facts_after, facts_before, "fact count must match");
    assert_eq!(events_after, events_before, "event count must match");
    assert_eq!(
        sample_after, sample_before,
        "canonical 10-row fact sample must match"
    );

    pool.close().await;
}
