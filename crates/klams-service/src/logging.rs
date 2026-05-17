//! Tracing/logging setup.
//!
//! Defaults to JSON output for `format = "json"` and pretty for any
//! other value. Filters can be overridden via `RUST_LOG`.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init(format: &str, default_level: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    if format.eq_ignore_ascii_case("json") {
        let layer = fmt::layer().json().with_current_span(false);
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init();
    } else {
        let layer = fmt::layer().compact();
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(layer)
            .try_init();
    }
}
