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

use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use ulid::Ulid;

use klams_store::backup as store_backup;
use klams_store::backup::{ArtifactKind, BackupArtifact, BackupError};
use klams_types::SameDayStrategy;

use self::lifecycle::{BackupRun, LockfileError};

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
#[derive(Debug, Clone)]
pub struct RunningSnapshot {
    pub run_id: Ulid,
    pub started_at: DateTime<Utc>,
    /// Mean of the last 5 successful run durations, projected
    /// forward from `started_at`. `None` on the first ever run.
    pub expected_end_at: Option<DateTime<Utc>>,
}

/// Maintenance-window flag shared between the backup orchestrator
/// and the axum middleware. `active()` is the hot-path read; clone
/// is cheap (two `Arc` bumps).
#[derive(Debug, Clone, Default)]
pub struct MaintenanceState {
    active: Arc<AtomicBool>,
    inflight: Arc<RwLock<Option<RunningSnapshot>>>,
}

impl MaintenanceState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Hot-path read used by the middleware on every non-GET request.
    #[inline]
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Cheap clone of the in-flight snapshot (held under `RwLock`).
    pub fn inflight(&self) -> Option<RunningSnapshot> {
        self.inflight.read().ok().and_then(|g| g.clone())
    }

    /// Mark a backup as in flight. The orchestrator's lockfile
    /// guarantees only one caller hits this at a time.
    pub fn mark_active(&self, snapshot: RunningSnapshot) {
        if let Ok(mut g) = self.inflight.write() {
            *g = Some(snapshot);
        }
        self.active.store(true, Ordering::Relaxed);
    }

    /// Clear in-flight state on backup completion (success or failure).
    pub fn clear(&self) {
        self.active.store(false, Ordering::Relaxed);
        if let Ok(mut g) = self.inflight.write() {
            *g = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
