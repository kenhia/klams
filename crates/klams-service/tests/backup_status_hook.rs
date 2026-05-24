//! Sprint 006 T042 + T043 — `status_hook` invocation contract.
//!
//! Exercises `klams_service::backup::hook::invoke` directly against
//! shell-script fixtures (no Docker required). Covers FR-009 happy
//! path, the SC-004 latency budget, FR-010 timeout / missing-exec /
//! exit-1 hooks, and verifies the orchestrator-supplied env vars.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use klams_service::backup::hook::{invoke, BackupHookEvent, HookEventKind};
use tempfile::tempdir;
use ulid::Ulid;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/backup/sample-hook.sh")
        .canonicalize()
        .expect("canonicalize fixture path")
}

fn ensure_executable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn make_event(kind: HookEventKind, run_id: Ulid) -> BackupHookEvent {
    let is_started = matches!(kind, HookEventKind::Started);
    BackupHookEvent {
        schema_version: 1,
        run_id: run_id.to_string(),
        event: kind,
        started_at: Utc::now(),
        ended_at: if is_started { None } else { Some(Utc::now()) },
        duration_ms: if is_started { None } else { Some(100) },
        artifacts: Vec::new(),
        ok: matches!(kind, HookEventKind::Finished),
        error: matches!(kind, HookEventKind::Failed).then(|| "boom".to_string()),
    }
}

#[tokio::test]
async fn sample_hook_writes_payload_with_env_for_started_and_finished() {
    let fixture = fixture_path();
    ensure_executable(&fixture);
    let out_dir = tempdir().unwrap();
    std::env::set_var("KLAMS_HOOK_OUT_DIR", out_dir.path());

    let run_id = Ulid::new();
    let started = make_event(HookEventKind::Started, run_id);
    let finished_inner = BackupHookEvent {
        schema_version: 1,
        run_id: run_id.to_string(),
        event: HookEventKind::Finished,
        started_at: started.started_at,
        ended_at: Some(Utc::now()),
        duration_ms: Some(123),
        artifacts: Vec::new(),
        ok: true,
        error: None,
    };

    // Measure the gap between "hook invocation begins" and the child
    // actually spawning (proxy for the SC-004 < 2s budget — we leave
    // headroom for the kpidash shim by demanding < 500ms here).
    let t0 = Instant::now();
    let r = invoke(Some(&fixture), Duration::from_secs(2), &started).await;
    let elapsed = t0.elapsed();
    assert!(r.ok, "started hook must succeed: {r:?}");
    assert!(
        elapsed < Duration::from_millis(500),
        "started hook end-to-end took {elapsed:?} (>= 500ms budget)"
    );

    let r = invoke(Some(&fixture), Duration::from_secs(2), &finished_inner).await;
    assert!(r.ok, "finished hook must succeed: {r:?}");

    // Drain stderr from set_var pollution doesn't matter; assert files.
    let started_path = out_dir
        .path()
        .join(format!("klams-hook-started-{run_id}.json"));
    let finished_path = out_dir
        .path()
        .join(format!("klams-hook-finished-{run_id}.json"));
    assert!(started_path.exists(), "started payload missing");
    assert!(finished_path.exists(), "finished payload missing");

    let started_body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&started_path).unwrap()).unwrap();
    let finished_body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&finished_path).unwrap()).unwrap();
    assert_eq!(started_body["schema_version"], 1);
    assert_eq!(started_body["event"], "started");
    assert_eq!(started_body["run_id"], run_id.to_string());
    assert_eq!(finished_body["event"], "finished");
    assert_eq!(finished_body["ok"], true);
    assert_eq!(finished_body["run_id"], run_id.to_string());
}

#[tokio::test]
async fn timeout_hook_is_killed_within_grace_and_returns_observability() {
    let dir = tempdir().unwrap();
    let script = dir.path().join("sleeping-hook.sh");
    std::fs::write(&script, "#!/usr/bin/env bash\nsleep 600\n").unwrap();
    ensure_executable(&script);

    let ev = make_event(HookEventKind::Finished, Ulid::new());
    let t0 = Instant::now();
    let r = invoke(Some(&script), Duration::from_millis(200), &ev).await;
    let elapsed = t0.elapsed();
    assert!(!r.ok, "stalled hook must report ok=false");
    assert!(r.timed_out, "stalled hook must mark timed_out=true");
    assert!(
        elapsed < Duration::from_secs(3),
        "SIGTERM+grace+SIGKILL must complete inside ~2s grace, got {elapsed:?}"
    );
}

#[tokio::test]
async fn missing_executable_returns_error_but_does_not_panic() {
    let path = PathBuf::from("/nonexistent/klams/hook-does-not-exist");
    let ev = make_event(HookEventKind::Started, Ulid::new());
    let r = invoke(Some(&path), Duration::from_secs(1), &ev).await;
    assert!(!r.ok);
    assert!(!r.timed_out);
    assert!(r.error.is_some());
}

#[tokio::test]
async fn exit_one_hook_reports_failure_without_panicking() {
    let dir = tempdir().unwrap();
    let script = dir.path().join("failing-hook.sh");
    std::fs::write(&script, "#!/usr/bin/env bash\nexit 1\n").unwrap();
    ensure_executable(&script);

    let ev = make_event(HookEventKind::Finished, Ulid::new());
    let r = invoke(Some(&script), Duration::from_secs(2), &ev).await;
    assert!(!r.ok);
    assert!(!r.timed_out);
    assert_eq!(r.exit_code, Some(1));
}
