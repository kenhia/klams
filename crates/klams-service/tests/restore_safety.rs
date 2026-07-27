//! Sprint 006 T030 (US2) — `restore::run_from` safety checks (FR-013).
//!
//! * A restore against a non-empty Postgres without `force=true`
//!   returns `Err(NonEmptyTarget)` and leaves the target untouched.
//! * The same call with `force=true` succeeds.
//! * A truncated dump file fails `pg_restore`. Because we pass
//!   `--single-transaction`, the failed restore rolls back and the
//!   target's pre-call rows are still present.
//!
//! `cargo test -p klams-service --test restore_safety -- --ignored`.

mod common;

use common::fixture::{generate_with_seed, FixtureScale};
use common::{seed, TestServer};
use klams_service::backup::restore::{run_from, RestoreError};
use klams_service::backup::{run_once, MaintenanceState, OrchestratorDeps};
use klams_store::QdrantStore;
use klams_types::SameDayStrategy;
use tempfile::tempdir;

const TEST_COLLECTION: &str = "klams_restore_safety_test";

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

/// See `restore_roundtrip::pg_bin_dir` — same rationale.
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

fn deps_for(dir: &std::path::Path, state: &MaintenanceState) -> OrchestratorDeps {
    OrchestratorDeps {
        backup_dir: dir.to_path_buf(),
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
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker-compose.test.yml"]
async fn restore_refuses_non_empty_without_force() {
    // Whole-database restore — see `common::whole_database_guard`.
    let _serial = common::whole_database_guard().await;
    ensure_collection().await;
    let dir = tempdir().unwrap();
    let state = MaintenanceState::new();
    let server = TestServer::spawn().await;
    let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
    sqlx::query("TRUNCATE facts, events, summaries CASCADE")
        .execute(&pool)
        .await
        .unwrap();

    let fixture = generate_with_seed(FixtureScale::tiny(), 0xDA70_0006_0030_0001);
    seed::load(&server.store, &fixture).await;
    let deps = deps_for(dir.path(), &state);
    run_once(&deps).await.expect("backup ran");

    let count_before = count_facts(&pool).await;
    assert!(count_before > 0, "fixture must produce facts");
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let err = run_from(&deps, &date, false, |_| {})
        .await
        .expect_err("restore without force must refuse");
    assert!(
        matches!(err, RestoreError::NonEmptyTarget { .. }),
        "expected NonEmptyTarget, got {err:?}"
    );

    // Untouched — counts unchanged.
    assert_eq!(count_facts(&pool).await, count_before);
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker-compose.test.yml"]
async fn restore_with_force_overwrites_non_empty() {
    // Whole-database restore — see `common::whole_database_guard`.
    let _serial = common::whole_database_guard().await;
    ensure_collection().await;
    let dir = tempdir().unwrap();
    let state = MaintenanceState::new();
    let server = TestServer::spawn().await;
    let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
    sqlx::query("TRUNCATE facts, events, summaries CASCADE")
        .execute(&pool)
        .await
        .unwrap();

    let fixture = generate_with_seed(FixtureScale::tiny(), 0xDA70_0006_0030_0002);
    seed::load(&server.store, &fixture).await;
    let deps = deps_for(dir.path(), &state);
    run_once(&deps).await.expect("backup ran");
    let count_before = count_facts(&pool).await;
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Mutate so we can prove the restore actually overwrote.
    sqlx::query("DELETE FROM facts WHERE created_at IS NOT NULL")
        .execute(&pool)
        .await
        .ok();
    // Inject a row so target is non-empty.
    sqlx::query(
        "INSERT INTO facts (id, type, payload, payload_hash, source, author_id) \
         VALUES (gen_random_uuid(), 'UserFact', '{}'::jsonb, '\\x00'::bytea, 'User', $1)",
    )
    .bind(klams_types::SYSTEM_AUTHOR_ID)
    .execute(&pool)
    .await
    .expect("insert decoy");
    assert!(count_facts(&pool).await > 0);

    run_from(&deps, &date, true, |_| {})
        .await
        .expect("restore with force must succeed");

    let count_after = count_facts(&pool).await;
    assert_eq!(
        count_after, count_before,
        "force restore must reproduce count exactly"
    );
    pool.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires docker-compose.test.yml"]
async fn restore_truncated_dump_rolls_back() {
    // Whole-database restore — see `common::whole_database_guard`.
    let _serial = common::whole_database_guard().await;
    ensure_collection().await;
    let dir = tempdir().unwrap();
    let state = MaintenanceState::new();
    let server = TestServer::spawn().await;
    let pool = sqlx::PgPool::connect(&pg_url()).await.unwrap();
    sqlx::query("TRUNCATE facts, events, summaries CASCADE")
        .execute(&pool)
        .await
        .unwrap();

    let fixture = generate_with_seed(FixtureScale::tiny(), 0xDA70_0006_0030_0003);
    seed::load(&server.store, &fixture).await;
    let deps = deps_for(dir.path(), &state);
    run_once(&deps).await.expect("backup ran");
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let pg_path = dir.path().join(format!("postgres-{date}.dump"));
    assert!(
        pg_path.exists(),
        "expected pg dump at {}",
        pg_path.display()
    );

    // Truncate the dump in place so pg_restore fails inside the
    // --single-transaction wrapper. The pre-call rows must survive.
    let mut bytes = std::fs::read(&pg_path).unwrap();
    bytes.truncate(bytes.len() / 4);
    std::fs::write(&pg_path, &bytes).unwrap();

    let count_before = count_facts(&pool).await;
    assert!(count_before > 0);

    let err = run_from(&deps, &date, true, |_| {})
        .await
        .expect_err("truncated dump must fail");
    // Either pg_restore returned non-zero or qdrant complained
    // about the still-good snapshot — pg is what we care about.
    let msg = err.to_string();
    assert!(
        msg.contains("pg_restore") || msg.contains("backup"),
        "expected pg_restore failure, got {msg}"
    );

    // --single-transaction means the failed restore rolled back.
    assert_eq!(
        count_facts(&pool).await,
        count_before,
        "failed restore must leave target untouched"
    );
    pool.close().await;
}
