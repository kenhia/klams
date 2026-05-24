//! Sprint 006 T020 (US1) — scheduler firing within the configured
//! window. Pure `tokio::time::pause()` simulation; no docker required.

use chrono::{Datelike, Timelike, Utc};
use klams_service::backup::scheduler;
use klams_types::WindowStartUtc;

#[test]
fn next_window_strictly_after_now() {
    let now = Utc::now();
    let w = WindowStartUtc {
        hour: u8::try_from((now.hour() + 1) % 24).unwrap(),
        minute: 0,
    };
    let next = scheduler::next_window_instant(now, w);
    assert!(next > now, "next must be strictly in the future");
    assert!(
        (next - now).num_seconds() < 25 * 3600,
        "next must be within ~24h"
    );
    assert_eq!(next.hour(), u32::from(w.hour));
    assert_eq!(next.minute(), 0);
}

#[test]
fn skipping_to_tomorrow_when_window_already_passed() {
    let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 5, 23, 10, 0, 0).unwrap();
    let w = WindowStartUtc { hour: 7, minute: 0 };
    let next = scheduler::next_window_instant(now, w);
    assert_eq!(next.day(), 24);
    assert_eq!(next.hour(), 7);
    assert_eq!(next.minute(), 0);
}

#[test]
fn next_window_today_when_minutes_ahead() {
    let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 5, 23, 7, 0, 0).unwrap();
    let w = WindowStartUtc { hour: 7, minute: 5 };
    let next = scheduler::next_window_instant(now, w);
    assert_eq!(next.day(), 23);
    assert_eq!(next.hour(), 7);
    assert_eq!(next.minute(), 5);
}
