//! T022 — state diff coverage for `klams-monitor::state::diff`.
//!
//! Mirrors the table in `specs/003-non-agentic-writes/data-model.md` §7.

use klams_monitor::poll::UnitState;
use klams_monitor::state::{apply, diff, PollObservation, PreviousState, ServiceEventKind};

fn obs<'a>(
    svc: &'a str,
    host: &'a str,
    state: UnitState,
    version: Option<&'a str>,
) -> PollObservation<'a> {
    PollObservation {
        service: svc,
        host,
        current_state: state,
        current_version: version,
    }
}

#[test]
fn cold_start_active_emits_up() {
    let prev = PreviousState::default();
    let o = obs("qdrant", "h1", UnitState::Active, None);
    let p = diff(&prev, &o).expect("event");
    assert_eq!(p.event, ServiceEventKind::Up);
    assert_eq!(p.service, "qdrant");
    assert_eq!(p.host, "h1");
}

#[test]
fn cold_start_inactive_emits_down() {
    let prev = PreviousState::default();
    let o = obs("qdrant", "h1", UnitState::Inactive, None);
    assert_eq!(diff(&prev, &o).unwrap().event, ServiceEventKind::Down);
}

#[test]
fn active_to_inactive_emits_down() {
    let prev = PreviousState {
        state: Some(UnitState::Active),
        version: None,
    };
    let o = obs("qdrant", "h1", UnitState::Inactive, None);
    assert_eq!(diff(&prev, &o).unwrap().event, ServiceEventKind::Down);
}

#[test]
fn inactive_to_active_emits_up() {
    let prev = PreviousState {
        state: Some(UnitState::Inactive),
        version: None,
    };
    let o = obs("qdrant", "h1", UnitState::Active, None);
    assert_eq!(diff(&prev, &o).unwrap().event, ServiceEventKind::Up);
}

#[test]
fn steady_state_active_emits_nothing() {
    let prev = PreviousState {
        state: Some(UnitState::Active),
        version: None,
    };
    let o = obs("qdrant", "h1", UnitState::Active, None);
    assert!(diff(&prev, &o).is_none());
}

#[test]
fn steady_state_inactive_emits_nothing() {
    let prev = PreviousState {
        state: Some(UnitState::Inactive),
        version: None,
    };
    let o = obs("qdrant", "h1", UnitState::Inactive, None);
    assert!(diff(&prev, &o).is_none());
}

#[test]
fn version_change_emits_version_changed() {
    let prev = PreviousState {
        state: Some(UnitState::Active),
        version: Some("1.0".into()),
    };
    let o = obs("qdrant", "h1", UnitState::Active, Some("1.1"));
    let p = diff(&prev, &o).expect("event");
    assert_eq!(p.event, ServiceEventKind::VersionChanged);
    assert_eq!(p.version.as_deref(), Some("1.1"));
}

#[test]
fn version_unchanged_emits_nothing() {
    let prev = PreviousState {
        state: Some(UnitState::Active),
        version: Some("1.0".into()),
    };
    let o = obs("qdrant", "h1", UnitState::Active, Some("1.0"));
    assert!(diff(&prev, &o).is_none());
}

#[test]
fn apply_updates_cache() {
    let mut prev = PreviousState::default();
    let o = obs("qdrant", "h1", UnitState::Active, Some("1.0"));
    apply(&mut prev, &o);
    assert_eq!(prev.state, Some(UnitState::Active));
    assert_eq!(prev.version.as_deref(), Some("1.0"));
}
