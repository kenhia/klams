//! Sprint 007 T028 — maintenance-window envelope.
//!
//! Asserts that `maintenance::check` short-circuits writes with the
//! canonical `MAINTENANCE_WINDOW_ACTIVE` envelope while the
//! `MaintenanceState` is active, and lets writes proceed otherwise.

use klams_mcp::{
    errors::MAINTENANCE_WINDOW_ACTIVE,
    maintenance::{check, RETRY_AFTER_SECONDS},
};
use klams_types::{MaintenanceState, RunningSnapshot};

fn snap() -> RunningSnapshot {
    RunningSnapshot {
        run_id: ulid::Ulid::new(),
        started_at: chrono::Utc::now(),
        expected_end_at: None,
    }
}

#[test]
fn writes_short_circuit_when_window_active() {
    let state = MaintenanceState::default();
    state.mark_active(snap());
    let env = check(&state).expect("expected MAINTENANCE_WINDOW_ACTIVE envelope");
    assert!(env.is_error);
    assert_eq!(env.meta.error_code, MAINTENANCE_WINDOW_ACTIVE);
    assert_eq!(env.meta.retry_after_seconds, Some(RETRY_AFTER_SECONDS));
}

#[test]
fn writes_proceed_when_window_inactive() {
    let state = MaintenanceState::default();
    assert!(check(&state).is_none());
}
