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
use klams_store::QdrantStore;
use klams_types::SameDayStrategy;
use tempfile::tempdir;

const TEST_COLLECTION: &str = "klams_restore_roundtrip_test";

fn pg_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into())
}

fn qdrant_grpc_url() -> String {
    std::env::var("TEST_QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:56334".into())
}

fn qdrant_rest_url() -> String {
    std::env::var("TEST_QDRANT_REST_URL").unwrap_or_else(|_| "http://127.0.0.1:56333".into())
}

async fn ensure_collection() {
    // Sprint 031 (#679/#687): DROP then create, rather than
    // create-if-missing.
    //
    // Restoring a snapshot into a collection leaves qdrant 1.18 unable
    // to create new snapshots from it — every later run of this file
    // then fails with `Failed to get_snapshot_creator, Failed to
    // restore local replica`, surfacing two steps downstream as a
    // baffling `ArtifactMissing { kind: Qdrant }`. Because the test
    // stack is long-lived, one restore run poisons every run after it
    // until somebody deletes the collection by hand.
    //
    // These collections hold nothing but this file's fixture, so
    // dropping is free.
    let rest = qdrant_rest_url();
    let _ = reqwest::Client::new()
        .delete(format!("{rest}/collections/{TEST_COLLECTION}"))
        .send()
        .await;
    QdrantStore::connect(&qdrant_grpc_url(), TEST_COLLECTION, 384)
        .await
        .expect("create collection");
}

/// Return the pg-16 client tools directory when present, so the
/// backup module picks the version-matched `pg_dump`/`pg_restore`
/// against the test compose's postgres:16. Host `pg_dump` 18 would
/// emit `transaction_timeout` SET statements that pg 16 rejects.
fn pg_bin_dir() -> Option<std::path::PathBuf> {
    let candidate = std::path::PathBuf::from("/usr/lib/postgresql/16/bin");
    candidate.is_dir().then_some(candidate)
}

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
    ensure_collection().await;
    let dir = tempdir().unwrap();
    let state = MaintenanceState::new();

    // Build a TestServer just so we have a CompositeStore wired to
    // the test stack for seeding. We don't drive HTTP.
    let server = TestServer::spawn().await;

    let pool = sqlx::PgPool::connect(&pg_url()).await.expect("pg connect");
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
        pg_url: pg_url(),
        pg_bin_dir: pg_bin_dir(),
        qdrant_rest_url: qdrant_rest_url(),
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
