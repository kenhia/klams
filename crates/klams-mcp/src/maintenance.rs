//! Maintenance-window envelope helper (sprint 007 R-009).
//!
//! Every MCP write tool entry must consult [`MaintenanceState::is_active`]
//! before doing any work. When a backup window is active the tool
//! returns [`maintenance_envelope`] instead of executing — the standard
//! MCP error envelope with code `MAINTENANCE_WINDOW_ACTIVE` and a
//! 30-second `retry_after_seconds` hint.

use crate::errors::{envelope_with_retry, ErrorEnvelope, MAINTENANCE_WINDOW_ACTIVE};
use klams_types::MaintenanceState;

/// Retry hint surfaced to MCP clients when a backup window is active.
/// Aligned with R-009; the value is intentionally short — backup
/// windows are minutes, not hours.
pub const RETRY_AFTER_SECONDS: u64 = 30;

/// Returns `Some(envelope)` if a maintenance window is active and the
/// tool MUST short-circuit; returns `None` if the tool is clear to run.
#[must_use]
pub fn check(state: &MaintenanceState) -> Option<ErrorEnvelope> {
    if state.active() {
        Some(maintenance_envelope())
    } else {
        None
    }
}

/// Build the canonical maintenance-window error envelope.
#[must_use]
pub fn maintenance_envelope() -> ErrorEnvelope {
    envelope_with_retry(
        MAINTENANCE_WINDOW_ACTIVE,
        "klams is in a maintenance window; retry shortly",
        RETRY_AFTER_SECONDS,
    )
}
