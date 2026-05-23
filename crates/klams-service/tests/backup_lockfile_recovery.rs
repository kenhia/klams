//! Sprint 006 T024a (US1) — stale-lockfile recovery edge case
//! ("Service restart mid-backup"). Filesystem only; no docker.

use chrono::Utc;
use klams_service::backup::lifecycle::{recover_stale_lock, LockfileContents, LOCKFILE_NAME};
use tempfile::tempdir;
use tokio::fs;
use ulid::Ulid;

#[tokio::test]
async fn recovers_after_dead_pid_clears_lockfile_and_partials() {
    let dir = tempdir().unwrap();

    // Seed: a stale lockfile pointing at a dead pid, two .partial files
    // from an interrupted run, and one committed file that must NOT be
    // touched.
    let dead_pid: u32 = 999_999; // very unlikely to be live
    let contents = LockfileContents {
        pid: dead_pid,
        run_id: Ulid::new().to_string(),
        started_at: Utc::now(),
    };
    fs::write(
        dir.path().join(LOCKFILE_NAME),
        serde_json::to_vec(&contents).unwrap(),
    )
    .await
    .unwrap();
    fs::write(dir.path().join("postgres-2026-05-23.dump.partial"), b"x")
        .await
        .unwrap();
    fs::write(dir.path().join("qdrant-2026-05-23.snapshot.partial"), b"x")
        .await
        .unwrap();
    fs::write(dir.path().join("postgres-2026-05-22.dump"), b"committed")
        .await
        .unwrap();

    let recovered = recover_stale_lock(dir.path()).await.unwrap();
    assert!(recovered.is_some(), "recovery should fire");
    let r = recovered.unwrap();
    assert_eq!(r.pid, dead_pid);

    // (b) lockfile removed.
    assert!(!dir.path().join(LOCKFILE_NAME).exists());
    // (c) .partial files removed.
    assert!(!dir.path().join("postgres-2026-05-23.dump.partial").exists());
    assert!(!dir
        .path()
        .join("qdrant-2026-05-23.snapshot.partial")
        .exists());
    // Committed file preserved.
    assert!(dir.path().join("postgres-2026-05-22.dump").exists());
    // NOTE (d) klams_backup_runs_total{ok="false"} increment is wired
    // in main.rs after recover_stale_lock returns; covered by the
    // service-startup smoke once Phase 6 lands the status_hook (FR-019).
    // NOTE (a) status_hook(`failed`, `service_restarted_mid_backup`)
    // assertion is deferred to Phase 6 (T041-T047) when the hook
    // executor lands. The recovery here is hook-free.
}

#[tokio::test]
async fn recovery_with_live_pid_is_refused() {
    let dir = tempdir().unwrap();
    let contents = LockfileContents {
        pid: std::process::id(),
        run_id: Ulid::new().to_string(),
        started_at: Utc::now(),
    };
    fs::write(
        dir.path().join(LOCKFILE_NAME),
        serde_json::to_vec(&contents).unwrap(),
    )
    .await
    .unwrap();
    fs::write(dir.path().join("postgres-2026-05-23.dump.partial"), b"x")
        .await
        .unwrap();

    let recovered = recover_stale_lock(dir.path()).await.unwrap();
    assert!(
        recovered.is_none(),
        "recovery must refuse when the lockfile pid is alive"
    );
    // Live owner's lockfile + partial files preserved.
    assert!(dir.path().join(LOCKFILE_NAME).exists());
    assert!(dir.path().join("postgres-2026-05-23.dump.partial").exists());
}
