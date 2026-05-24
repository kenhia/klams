//! Backup feature Prometheus series (sprint 006 T012).
//!
//! Names + labels per `specs/006-maintenance-and-backups/data-model.md`
//! "Prometheus series schema". All five series are described once at
//! service start so they appear in `/metrics` before the first
//! backup runs.

use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};

pub const LAST_SUCCESS_TS: &str = "klams_backup_last_success_timestamp_seconds";
pub const DURATION_SECONDS: &str = "klams_backup_duration_seconds";
pub const RUNS_TOTAL: &str = "klams_backup_runs_total";
pub const HOOK_INVOCATIONS_TOTAL: &str = "klams_backup_hook_invocations_total";
pub const MAINTENANCE_ACTIVE: &str = "klams_maintenance_mode_active";

/// Register descriptions with the global recorder. Safe to call
/// repeatedly.
pub fn describe() {
    describe_gauge!(
        LAST_SUCCESS_TS,
        "Unix timestamp of the most recent successful backup run (sprint 006 FR-006)"
    );
    describe_histogram!(
        DURATION_SECONDS,
        "Per-artifact backup duration in seconds, labelled by kind (sprint 006 FR-006)"
    );
    describe_counter!(
        RUNS_TOTAL,
        "Backup run completions, labelled by outcome (sprint 006 FR-006)"
    );
    describe_counter!(
        HOOK_INVOCATIONS_TOTAL,
        "status_hook invocations, labelled by event + invocation outcome (sprint 006 FR-006)"
    );
    describe_gauge!(
        MAINTENANCE_ACTIVE,
        "1 while a backup is in flight, 0 otherwise (sprint 006 FR-007)"
    );
}

/// Mirror `MaintenanceState::active` onto the gauge.
pub fn set_maintenance_active(active: bool) {
    gauge!(MAINTENANCE_ACTIVE).set(f64::from(u8::from(active)));
}

/// Record one finished `BackupRun`.
pub fn incr_runs_total(ok: bool) {
    counter!(RUNS_TOTAL, "ok" => bool_label(ok)).increment(1);
}

/// Stamp the most recent success.
#[allow(clippy::cast_precision_loss)]
pub fn record_last_success(unix_seconds: u64) {
    gauge!(LAST_SUCCESS_TS).set(unix_seconds as f64);
}

/// Observe a per-artifact duration.
pub fn observe_duration(kind: &'static str, seconds: f64) {
    histogram!(DURATION_SECONDS, "kind" => kind).record(seconds);
}

/// Increment the hook invocations counter.
pub fn incr_hook_invocations(event: &'static str, ok: bool) {
    counter!(HOOK_INVOCATIONS_TOTAL, "event" => event, "ok" => bool_label(ok)).increment(1);
}

const fn bool_label(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_is_idempotent() {
        describe();
        describe();
    }

    #[test]
    fn helpers_compile_and_dispatch() {
        set_maintenance_active(true);
        set_maintenance_active(false);
        incr_runs_total(true);
        incr_runs_total(false);
        record_last_success(1_700_000_000);
        observe_duration("postgres", 12.5);
        observe_duration("qdrant", 4.2);
        incr_hook_invocations("started", true);
        incr_hook_invocations("finished", false);
    }
}
