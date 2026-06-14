//! klams-monitor library surface.

pub mod poll;
pub mod publish;
pub mod state;

#[cfg(feature = "kpidash")]
pub mod kpidash;

#[must_use]
pub fn banner() -> &'static str {
    "klams-monitor ready"
}
