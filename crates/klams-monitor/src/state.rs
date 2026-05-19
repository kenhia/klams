//! In-memory state cache and diff function for the monitor.
//!
//! On each tick we call [`crate::poll::is_active`] for every watched
//! unit and feed `(prev, current)` into [`diff`]. The result is
//! [`Some(ServiceEventPayload)`](ServiceEventPayload) when a transition
//! is worth recording and `None` for steady-state polls. See
//! `specs/003-non-agentic-writes/data-model.md` §7.

use crate::poll::UnitState;
use serde::{Deserialize, Serialize};

/// Previous observation for a single unit. `None` for `state` means
/// "never observed" (cold start, first tick). `version` is the most
/// recent version string we've seen, regardless of state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviousState {
    pub state: Option<UnitState>,
    pub version: Option<String>,
}

/// JSON shape posted to `POST /memory/events` with `category=Service`.
/// Mirrors `ServiceEventValidator` in `klams-core`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceEventPayload {
    pub service: String,
    pub event: ServiceEventKind,
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceEventKind {
    Up,
    Down,
    Restart,
    VersionChanged,
}

/// Inputs available to the diff function on each tick. Holding these
/// as a struct keeps the call-site tidy and makes the addition of new
/// signals (e.g. config-hash diff) a non-breaking change.
#[derive(Debug, Clone)]
pub struct PollObservation<'a> {
    pub service: &'a str,
    pub host: &'a str,
    pub current_state: UnitState,
    pub current_version: Option<&'a str>,
}

/// Pure: compares `prev` with `obs` and returns the event payload to
/// post (or `None` if nothing changed). Always update `prev` after
/// calling this regardless of the result.
#[must_use]
pub fn diff(prev: &PreviousState, obs: &PollObservation<'_>) -> Option<ServiceEventPayload> {
    // Version-changed wins over state-only events, but only fires when
    // both sides have a version AND they differ.
    if let (Some(p), Some(c)) = (prev.version.as_deref(), obs.current_version) {
        if p != c {
            return Some(ServiceEventPayload {
                service: obs.service.into(),
                event: ServiceEventKind::VersionChanged,
                host: obs.host.into(),
                version: Some(c.into()),
                port: None,
            });
        }
    }
    let event = match (prev.state, obs.current_state) {
        (None | Some(UnitState::Inactive), UnitState::Active) => ServiceEventKind::Up,
        (None | Some(UnitState::Active), UnitState::Inactive) => ServiceEventKind::Down,
        // Steady state: no event.
        (Some(UnitState::Active), UnitState::Active)
        | (Some(UnitState::Inactive), UnitState::Inactive) => return None,
    };
    Some(ServiceEventPayload {
        service: obs.service.into(),
        event,
        host: obs.host.into(),
        version: obs.current_version.map(str::to_owned),
        port: None,
    })
}

/// Convenience: apply an observation to a `PreviousState` cache. Call
/// this after `diff` so the next tick has the latest snapshot.
pub fn apply(prev: &mut PreviousState, obs: &PollObservation<'_>) {
    prev.state = Some(obs.current_state);
    if let Some(v) = obs.current_version {
        prev.version = Some(v.to_owned());
    }
}
