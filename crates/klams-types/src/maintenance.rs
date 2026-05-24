//! Sprint 006: maintenance-window flag shared between the backup
//! orchestrator (klams-service) and the HTTP middleware (klams-api).
//!
//! Lives in `klams-types` so klams-api can read it without taking a
//! cyclic dependency on klams-service.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hot-path read used by the middleware on every non-GET request.
    #[inline]
    #[must_use]
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Cheap clone of the in-flight snapshot (held under `RwLock`).
    #[must_use]
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

/// Wire shape of `maintenance` block on `/healthz` (FR-018).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceSnapshot {
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_end_at: Option<DateTime<Utc>>,
}

impl MaintenanceSnapshot {
    #[must_use]
    pub fn from_state(state: &MaintenanceState) -> Self {
        let active = state.active();
        match state.inflight() {
            Some(s) => Self {
                active,
                run_id: Some(s.run_id.to_string()),
                started_at: Some(s.started_at),
                expected_end_at: s.expected_end_at,
            },
            None => Self {
                active,
                run_id: None,
                started_at: None,
                expected_end_at: None,
            },
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

    #[test]
    fn snapshot_from_inactive_state_serializes_minimally() {
        let s = MaintenanceState::new();
        let snap = MaintenanceSnapshot::from_state(&s);
        let v = serde_json::to_value(&snap).unwrap();
        assert_eq!(v["active"], false);
        assert!(v.get("run_id").is_none());
    }

    #[test]
    fn snapshot_from_active_state_includes_run_id() {
        let s = MaintenanceState::new();
        s.mark_active(snap());
        let snap = MaintenanceSnapshot::from_state(&s);
        assert!(snap.active);
        assert!(snap.run_id.is_some());
        assert!(snap.started_at.is_some());
    }
}
