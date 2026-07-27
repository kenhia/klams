//! Time-window validation shared by every windowed read.
//!
//! Sprint 031 (#645). `GET /v1/memories` and the `event_search` MCP
//! tool each carried their own copy of these two checks, down to
//! character-identical message strings. Two copies of a rule is two
//! places to fix it and one place to forget — and the REST and MCP
//! answers to the same bad window are supposed to agree, since they
//! read the same `memories_max_window_days` config.
//!
//! The error *types* stay per-surface (`ApiError` vs the MCP
//! `ErrorEnvelope`): only the rule and its wording live here.

use chrono::{DateTime, Duration, Utc};

/// Why a requested window was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowError {
    /// `since` is after `until`.
    Inverted {
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    },
    /// The window is wider than the configured ceiling.
    TooLarge { max_days: u32 },
}

impl WindowError {
    /// Operator-facing description. Says what was wrong and what the
    /// bound is, so the caller can fix the request without reading the
    /// server config.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Inverted { since, until } => format!(
                "window is inverted: since ({}) is after until ({})",
                since.to_rfc3339(),
                until.to_rfc3339()
            ),
            Self::TooLarge { max_days } => {
                format!("requested window exceeds configured maximum of {max_days} days")
            }
        }
    }
}

impl std::fmt::Display for WindowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// Check `[since, until]` against `max_days`.
///
/// # Errors
/// [`WindowError::Inverted`] when `since > until`;
/// [`WindowError::TooLarge`] when the span exceeds `max_days`.
pub fn validate_window(
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    max_days: u32,
) -> Result<(), WindowError> {
    if since > until {
        return Err(WindowError::Inverted { since, until });
    }
    if (until - since) > Duration::days(i64::from(max_days)) {
        return Err(WindowError::TooLarge { max_days });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(offset_hours: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + offset_hours * 3600, 0).expect("timestamp")
    }

    #[test]
    fn accepts_a_window_inside_the_ceiling() {
        assert!(validate_window(t(0), t(23), 1).is_ok());
    }

    #[test]
    fn accepts_a_window_exactly_at_the_ceiling() {
        // The bound is inclusive — a caller asking for exactly the
        // advertised maximum must not be refused.
        assert!(validate_window(t(0), t(24), 1).is_ok());
    }

    #[test]
    fn rejects_an_inverted_window_and_says_which_way() {
        let err = validate_window(t(5), t(1), 30).expect_err("inverted");
        assert!(matches!(err, WindowError::Inverted { .. }));
        assert!(err.message().contains("window is inverted"));
    }

    #[test]
    fn rejects_a_window_past_the_ceiling_and_names_it() {
        let err = validate_window(t(0), t(25), 1).expect_err("too large");
        assert_eq!(err, WindowError::TooLarge { max_days: 1 });
        assert!(err.message().contains("maximum of 1 days"));
    }
}
