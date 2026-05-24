//! Sprint 006 T016 (US1) — `pg_dump` integration against the test compose
//! Postgres. Requires `docker-compose.test.yml` to be running.
//!
//! `cargo test -p klams-service --test backup_pg_dump -- --ignored`.

use klams_store::backup::postgres;
use tempfile::tempdir;

fn pg_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into())
}

/// Optional pg-16 client tools directory, when present on the host
/// (the test compose runs postgres:16; host `pg_dump` 18 would emit
/// `transaction_timeout` SETs which pg 16 rejects).
fn pg_bin_dir() -> Option<std::path::PathBuf> {
    let candidate = std::path::PathBuf::from("/usr/lib/postgresql/16/bin");
    candidate.is_dir().then_some(candidate)
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn dump_writes_atomic_artifact_and_no_partial_on_success() {
    let dir = tempdir().unwrap();
    let date = "2026-05-23";
    let artifact = postgres::dump(dir.path(), &pg_url(), date, None, pg_bin_dir().as_deref())
        .await
        .expect("dump succeeded");

    assert_eq!(artifact.path, dir.path().join("postgres-2026-05-23.dump"));
    assert!(artifact.path.exists(), "committed dump missing");
    assert!(
        !dir.path().join("postgres-2026-05-23.dump.partial").exists(),
        ".partial must be removed after success"
    );
    assert!(artifact.ok);
    assert!(artifact.bytes > 0, "expected non-empty dump");
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn dump_failure_leaves_no_committed_file() {
    let dir = tempdir().unwrap();
    let bad_url = "postgres://klams:wrong-password@127.0.0.1:55432/klams";
    let err = postgres::dump(
        dir.path(),
        bad_url,
        "2026-05-23",
        None,
        pg_bin_dir().as_deref(),
    )
    .await
    .expect_err("bad credentials should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("pg_dump") || msg.contains("authentication") || msg.contains("password"),
        "unexpected error: {msg}"
    );
    assert!(
        !dir.path().join("postgres-2026-05-23.dump").exists(),
        "no committed file on failure"
    );
}
