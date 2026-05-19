//! klams-scanner library surface.
//!
//! Public modules are added incrementally per sprint 003 tasks
//! (T034 walk, T035 chunk, T036 cursor, T037 publish, T039 metrics).
//! At T002 scaffold time the crate exposes only its name banner so
//! `cargo build --workspace --bins` passes.

/// Banner returned by the binary entry point on startup. Centralised
/// here so an eventual unit test can pin the wording.
#[must_use]
pub fn banner() -> &'static str {
    "klams-scanner ready"
}
