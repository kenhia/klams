//! Sprint 006 T019 (US1) — end-to-end `run_once` orchestrator.
//! Drives both backends from the test compose stack.
//! `cargo test -p klams-service --test backup_orchestrator -- --ignored`.

mod common;

use klams_service::backup::{run_once, MaintenanceState, OrchestratorDeps};
use klams_types::SameDayStrategy;
use tempfile::tempdir;

const TEST_COLLECTION: &str = "klams_backup_orchestrator_test";

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn run_once_produces_both_artifacts_and_clears_state() {
    common::ensure_collection(TEST_COLLECTION, false).await;
    let dir = tempdir().unwrap();
    let state = MaintenanceState::new();

    let deps = OrchestratorDeps {
        backup_dir: dir.path().to_path_buf(),
        pg_url: common::test_pg_url(),
        pg_bin_dir: common::pg16_bin_dir(),
        qdrant_rest_url: common::test_qdrant_rest_url(),
        qdrant_collection: TEST_COLLECTION.into(),
        daily_count: 14,
        weekly_count: 4,
        same_day_strategy: SameDayStrategy::Suffix,
        drop_remote_qdrant_snapshot: true,
        state: state.clone(),
        status_hook: None,
        status_hook_timeout: std::time::Duration::from_secs(10),
    };

    // Pre-condition: maintenance flag clear, no lockfile.
    assert!(!state.active());
    assert!(!dir.path().join("lockfile").exists());

    let run = run_once(&deps).await.expect("orchestrator ran");

    // Post-conditions: state cleared, lockfile removed, both
    // artifacts on disk.
    assert!(!state.active(), "maintenance flag must clear on exit");
    assert!(
        !dir.path().join("lockfile").exists(),
        "lockfile must be released"
    );

    let pg_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("postgres-")
                && std::path::Path::new(name.as_ref())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dump"))
        })
        .collect();
    let q_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("qdrant-")
                && std::path::Path::new(name.as_ref())
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("snapshot"))
        })
        .collect();
    assert_eq!(pg_files.len(), 1, "exactly one postgres dump expected");
    assert_eq!(q_files.len(), 1, "exactly one qdrant snapshot expected");

    assert_eq!(run.ok, Some(true), "run.ok must be Some(true)");
    assert_eq!(run.artifacts.len(), 2);
    assert!(run.duration_ms().is_some());
}
