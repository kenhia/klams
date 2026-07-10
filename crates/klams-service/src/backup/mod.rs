//! Backup orchestration (sprint 006).
//!
//! Owns the scheduler, the `BackupRun` lifecycle, the `status_hook`
//! executor, and the Prometheus metric registrations. Delegates the
//! snapshot/restore primitives to `klams_store::backup`.

pub mod hook;
pub mod lifecycle;
pub mod metrics;
pub mod restore;
pub mod scheduler;

use chrono::Utc;
use std::path::{Path, PathBuf};

use klams_store::backup as store_backup;
use klams_store::backup::{ArtifactKind, BackupArtifact, BackupError};
use klams_types::SameDayStrategy;

pub use klams_types::{MaintenanceSnapshot, MaintenanceState, RunningSnapshot};

use self::lifecycle::{BackupRun, LockfileError};

/// Sprint 020 — newest successful-backup timestamp recoverable from
/// disk, as unix seconds: the max mtime across `postgres-*.dump`
/// artifacts in `dir` (a successful run always writes one). Used to
/// seed the `klams_backup_last_success_timestamp_seconds` gauge at
/// startup so the "Last backup age" panel doesn't read No Data until
/// the next nightly run.
///
/// # Errors
/// Propagates the `read_dir` failure (missing/unreadable dir).
pub fn newest_backup_unix_seconds(dir: &Path) -> std::io::Result<Option<u64>> {
    let mut newest: Option<std::time::SystemTime> = None;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_pg_dump = name.starts_with("postgres-")
            && Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dump"));
        if !is_pg_dump {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        if newest.is_none_or(|n| mtime > n) {
            newest = Some(mtime);
        }
    }
    Ok(newest.map(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }))
}

/// All the inputs the orchestrator needs to perform one backup run.
#[derive(Debug, Clone)]
pub struct OrchestratorDeps {
    pub backup_dir: PathBuf,
    pub pg_url: String,
    pub pg_bin_dir: Option<PathBuf>,
    pub qdrant_rest_url: String,
    pub qdrant_collection: String,
    pub daily_count: u32,
    pub weekly_count: u32,
    pub same_day_strategy: SameDayStrategy,
    pub drop_remote_qdrant_snapshot: bool,
    pub state: MaintenanceState,
    /// Optional executable invoked at `started` / `finished` /
    /// `failed`. `None` disables the feature.
    pub status_hook: Option<PathBuf>,
    /// Wall-clock timeout for each `status_hook` invocation.
    pub status_hook_timeout: std::time::Duration,
}

/// Errors raised by [`run_once`].
#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error("lockfile: {0}")]
    Lockfile(#[from] LockfileError),
    #[error("backup: {0}")]
    Backup(#[from] BackupError),
}

/// Execute one backup run end-to-end (postgres dump → qdrant snapshot
/// → retention prune), maintaining lockfile, `MaintenanceState`, and
/// Prometheus metrics. Returns the completed [`BackupRun`].
///
/// The maintenance flag is cleared (and the lockfile released) on
/// every exit path — success, partial failure, or panic-free error.
///
/// # Errors
///
/// Returns [`OrchestratorError::Lockfile`] when another backup is
/// already in flight, and [`OrchestratorError::Backup`] for an
/// unrecoverable failure inside the run.
pub async fn run_once(deps: &OrchestratorDeps) -> Result<BackupRun, OrchestratorError> {
    let date_str = Utc::now().format("%Y-%m-%d").to_string();
    let mut run = BackupRun::start();

    let _lock_path = lifecycle::acquire_lock(&deps.backup_dir, run.run_id, run.started_at).await?;

    let snapshot = RunningSnapshot {
        run_id: run.run_id,
        started_at: run.started_at,
        expected_end_at: None,
    };
    deps.state.mark_active(snapshot);
    metrics::set_maintenance_active(true);

    // Fire `started` hook before any artifact work begins.
    let _ = hook::invoke(
        deps.status_hook.as_deref(),
        deps.status_hook_timeout,
        &hook::BackupHookEvent::started(&run),
    )
    .await;

    // Use a guard so the maintenance flag + lockfile are always cleared.
    let result = run_once_inner(deps, &mut run, &date_str).await;

    deps.state.clear();
    metrics::set_maintenance_active(false);
    if let Err(e) = lifecycle::release_lock(&deps.backup_dir).await {
        tracing::warn!(error = %e, "release_lock failed");
    }

    let ok = match &result {
        Ok(()) => {
            run.finish_ok();
            run.ok.unwrap_or(false)
        }
        Err(e) => {
            run.finish_err(e.to_string());
            false
        }
    };

    metrics::incr_runs_total(ok);
    if ok {
        let secs = u64::try_from(Utc::now().timestamp().max(0)).unwrap_or(0);
        metrics::record_last_success(secs);
    }

    // Fire `finished` or `failed` regardless of whether `started`
    // succeeded — hook failure is observability, not control flow.
    let terminal = if ok {
        hook::BackupHookEvent::finished(&run)
    } else {
        hook::BackupHookEvent::failed(&run)
    };
    let _ = hook::invoke(
        deps.status_hook.as_deref(),
        deps.status_hook_timeout,
        &terminal,
    )
    .await;

    result.map(|()| run.clone()).or(Ok(run))
}

async fn run_once_inner(
    deps: &OrchestratorDeps,
    run: &mut BackupRun,
    date_str: &str,
) -> Result<(), OrchestratorError> {
    let pg_suffix = next_suffix_for_date(
        &deps.backup_dir,
        ArtifactKind::Postgres,
        date_str,
        deps.same_day_strategy,
    );
    let pg_art = store_backup::postgres::dump(
        &deps.backup_dir,
        &deps.pg_url,
        date_str,
        pg_suffix,
        deps.pg_bin_dir.as_deref(),
    )
    .await;
    record_artifact(run, ArtifactKind::Postgres, pg_art);

    let q_suffix = next_suffix_for_date(
        &deps.backup_dir,
        ArtifactKind::Qdrant,
        date_str,
        deps.same_day_strategy,
    );
    let q_art = store_backup::qdrant::snapshot(
        &deps.backup_dir,
        &deps.qdrant_rest_url,
        &deps.qdrant_collection,
        date_str,
        q_suffix,
        deps.drop_remote_qdrant_snapshot,
    )
    .await;
    record_artifact(run, ArtifactKind::Qdrant, q_art);

    // Only prune after every artifact succeeded (FR-005 / R-006).
    let all_ok = run.artifacts.iter().all(|a| a.ok);
    if all_ok {
        if let Err(e) =
            store_backup::retention::prune(&deps.backup_dir, deps.daily_count, deps.weekly_count)
                .await
        {
            tracing::warn!(error = %e, "retention prune failed (artifacts still committed)");
        }
        Ok(())
    } else {
        Err(OrchestratorError::Backup(BackupError::QdrantSnapshot(
            run.error
                .clone()
                .unwrap_or_else(|| "artifact failed".into()),
        )))
    }
}

fn record_artifact(
    run: &mut BackupRun,
    kind: ArtifactKind,
    res: Result<BackupArtifact, BackupError>,
) {
    match res {
        Ok(a) => {
            #[allow(clippy::cast_precision_loss)]
            let secs = (a.duration_ms as f64) / 1000.0;
            metrics::observe_duration(kind.prefix(), secs);
            run.artifact_done(a);
        }
        Err(e) => {
            let msg = e.to_string();
            tracing::error!(?kind, error = %msg, "backup artifact failed");
            run.artifact_done(BackupArtifact {
                kind,
                path: PathBuf::new(),
                bytes: 0,
                duration_ms: 0,
                ok: false,
                error: Some(msg),
            });
        }
    }
}

/// If the same-day strategy is `Overwrite`, returns `None` (the
/// caller's existing file is replaced). For `Suffix`, returns the
/// next available `-N` suffix (or `None` if the un-suffixed slot
/// is still free).
fn next_suffix_for_date(
    backup_dir: &Path,
    kind: ArtifactKind,
    date_str: &str,
    strategy: SameDayStrategy,
) -> Option<u32> {
    if matches!(strategy, SameDayStrategy::Overwrite) {
        return None;
    }
    let prefix = kind.prefix();
    let ext = kind.extension();
    let base = format!("{prefix}-{date_str}.{ext}");
    if !backup_dir.join(&base).exists() {
        return None;
    }
    for n in 1..u32::MAX {
        let candidate = format!("{prefix}-{date_str}-{n}.{ext}");
        if !backup_dir.join(&candidate).exists() {
            return Some(n);
        }
    }
    None
}

/// Snapshot of the in-flight `BackupRun` shared between the
/// orchestrator and the maintenance-mode middleware / `/healthz`.
/// Re-exported from `klams_types` so HTTP middleware can read it
/// without depending on klams-service.
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use ulid::Ulid;

    fn snap() -> RunningSnapshot {
        RunningSnapshot {
            run_id: Ulid::new(),
            started_at: Utc::now(),
            expected_end_at: None,
        }
    }

    #[test]
    fn new_is_inactive() {
        let s = MaintenanceState::new();
        assert!(!s.active());
        assert!(s.inflight().is_none());
    }

    #[test]
    fn mark_active_then_clear() {
        let s = MaintenanceState::new();
        let snapshot = snap();
        let run_id = snapshot.run_id;
        s.mark_active(snapshot);
        assert!(s.active());
        assert_eq!(s.inflight().unwrap().run_id, run_id);
        s.clear();
        assert!(!s.active());
        assert!(s.inflight().is_none());
    }

    #[test]
    fn clone_shares_state() {
        let s = MaintenanceState::new();
        let s2 = s.clone();
        s.mark_active(snap());
        assert!(s2.active());
    }

    #[test]
    fn newest_backup_unix_seconds_picks_latest_postgres_dump() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Empty dir → no seed.
        assert_eq!(super::newest_backup_unix_seconds(dir.path()).unwrap(), None);
        // Non-matching files are ignored.
        std::fs::write(dir.path().join("lockfile"), b"x").unwrap();
        std::fs::write(dir.path().join("qdrant-2026-07-01.snapshot"), b"x").unwrap();
        assert_eq!(super::newest_backup_unix_seconds(dir.path()).unwrap(), None);
        // Two dumps → max mtime wins (set explicit mtimes far apart).
        let old = dir.path().join("postgres-2026-07-01.dump");
        let new = dir.path().join("postgres-2026-07-02.dump");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&new, b"x").unwrap();
        let t_old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        let t_new = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2_000_000);
        for (f, t) in [(&old, t_old), (&new, t_new)] {
            let dest = std::fs::File::options().write(true).open(f).unwrap();
            dest.set_modified(t).unwrap();
        }
        assert_eq!(
            super::newest_backup_unix_seconds(dir.path()).unwrap(),
            Some(2_000_000)
        );
    }
}
