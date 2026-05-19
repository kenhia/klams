//! klams-monitor library surface.

pub mod poll;
pub mod publish;
pub mod state;

#[must_use]
pub fn banner() -> &'static str {
    "klams-monitor ready"
}
