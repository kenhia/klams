//! klams-monitor library surface.
//!
//! Modules land incrementally: T026 poll, T027 state, T028 publish.
//! At T002 scaffold time the crate exposes only its banner.

#[must_use]
pub fn banner() -> &'static str {
    "klams-monitor ready"
}
