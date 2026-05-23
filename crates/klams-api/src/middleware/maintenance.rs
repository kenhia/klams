//! Maintenance-window middleware: 503 + Retry-After on non-critical
//! writes while `MaintenanceState::active()` is true (sprint 006 T037/T038).
