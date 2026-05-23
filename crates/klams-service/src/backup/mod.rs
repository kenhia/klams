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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use ulid::Ulid;

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
