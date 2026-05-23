//! UTC-time-based one-shot-per-day backup trigger (sprint 006 T026).
//!
//! Hand-rolled `tokio::time::sleep_until` loop. Computes the next UTC
//! instant matching `window_start_utc` (HH:MM), sleeps until then,
//! invokes `run_once` once per day. Testable under `tokio::time::pause()`.

use std::time::Duration;

use chrono::{DateTime, NaiveTime, TimeZone, Utc};
use klams_types::WindowStartUtc;

use super::{run_once, OrchestratorDeps};

/// Run the scheduler loop until cancelled. Computes the next UTC
/// firing instant, sleeps until then, invokes [`run_once`], then
/// schedules the day after that.
pub async fn run(deps: OrchestratorDeps, window: WindowStartUtc) {
    loop {
        let now = Utc::now();
        let next = next_window_instant(now, window);
        let wait = (next - now).to_std().unwrap_or(Duration::from_millis(0));
        tracing::info!(
            next = %next,
            wait_secs = wait.as_secs(),
            "backup scheduler sleeping until next window"
        );
        tokio::time::sleep(wait).await;

        match run_once(&deps).await {
            Ok(run) => tracing::info!(
                run_id = %run.run_id,
                ok = ?run.ok,
                duration_ms = ?run.duration_ms(),
                "backup run complete"
            ),
            Err(e) => tracing::error!(error = %e, "backup run failed"),
        }
    }
}

/// Compute the next UTC `DateTime` strictly after `from` whose
/// `HH:MM` matches `window`.
#[must_use]
pub fn next_window_instant(from: DateTime<Utc>, window: WindowStartUtc) -> DateTime<Utc> {
    let target_time = NaiveTime::from_hms_opt(u32::from(window.hour), u32::from(window.minute), 0)
        .expect("validated 0..=23 / 0..=59");
    let today = from.date_naive();
    let candidate_today = Utc.from_utc_datetime(&today.and_time(target_time));
    if candidate_today > from {
        candidate_today
    } else {
        let tomorrow = today + chrono::Days::new(1);
        Utc.from_utc_datetime(&tomorrow.and_time(target_time))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn next_window_today_when_in_future() {
        let from = Utc.with_ymd_and_hms(2026, 5, 23, 4, 0, 0).unwrap();
        let w = WindowStartUtc { hour: 7, minute: 0 };
        let next = next_window_instant(from, w);
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 23, 7, 0, 0).unwrap());
    }

    #[test]
    fn next_window_tomorrow_when_passed() {
        let from = Utc.with_ymd_and_hms(2026, 5, 23, 8, 30, 0).unwrap();
        let w = WindowStartUtc { hour: 7, minute: 0 };
        let next = next_window_instant(from, w);
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 24, 7, 0, 0).unwrap());
    }

    #[test]
    fn next_window_equal_now_goes_tomorrow() {
        let from = Utc.with_ymd_and_hms(2026, 5, 23, 7, 0, 0).unwrap();
        let w = WindowStartUtc { hour: 7, minute: 0 };
        let next = next_window_instant(from, w);
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 5, 24, 7, 0, 0).unwrap());
    }
}
