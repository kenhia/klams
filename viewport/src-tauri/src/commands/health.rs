//! Health command + background poll task.
//!
//! Spawns one tokio task per app instance that calls
//! `factory.health()` every `refresh_interval_seconds` (from
//! [`crate::config`]) and emits the `klams://health` Tauri event
//! with the parsed snapshot. On transport/5xx failure the loop
//! applies exponential backoff (start = configured interval, double
//! on each consecutive failure, cap = 60s) and resets to the
//! configured interval on the next success — see FR-028.

use crate::commands::{AppState, ViewportError};
use klams_types::HealthSnapshot;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::sleep;

pub const HEALTH_EVENT: &str = "klams://health";
pub const CONFIG_CHANGED_EVENT: &str = "klams://config-changed";
const BACKOFF_CAP: Duration = Duration::from_secs(60);

fn diag(msg: &str) {
    use std::io::Write;
    if !crate::debug_enabled() {
        return;
    }
    let path = std::env::temp_dir().join("klams-viewport.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "health: {msg}");
    }
}

#[tauri::command]
pub async fn get_health(
    state: tauri::State<'_, AppState>,
) -> Result<HealthSnapshot, ViewportError> {
    state.factory.health().await
}

/// Spawn the background health poll. Holds an `AppHandle` clone so
/// emitted events reach every window.
pub fn spawn_poll(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        diag("poll task started");
        let state = if let Some(s) = app.try_state::<AppState>() {
            s.inner().clone()
        } else {
            diag("poll: no AppState; exiting");
            return;
        };
        let mut consecutive_failures: u32 = 0;
        loop {
            let configured = Duration::from_secs(u64::from(
                crate::config::load().refresh_interval_seconds.max(1),
            ));
            diag(&format!(
                "poll: requesting health (interval={}s)",
                configured.as_secs()
            ));
            match state.factory.health().await {
                Ok(snap) => {
                    consecutive_failures = 0;
                    diag(&format!("poll: ok status={:?}", snap.status));
                    let _ = app.emit(HEALTH_EVENT, &snap);
                    sleep(configured).await;
                }
                Err(e) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    diag(&format!(
                        "poll: error consecutive={consecutive_failures} err={e}"
                    ));
                    let delay = backoff(configured, consecutive_failures);
                    sleep(delay).await;
                }
            }
        }
    });
}

fn backoff(base: Duration, attempt: u32) -> Duration {
    // attempt is 1-based on first failure.
    let factor: u32 = 1u32 << attempt.min(6); // cap shift at 6 → ×64
    let scaled = base.saturating_mul(factor);
    if scaled > BACKOFF_CAP {
        BACKOFF_CAP
    } else {
        scaled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_caps_at_60s() {
        let base = Duration::from_secs(10);
        assert_eq!(backoff(base, 1), Duration::from_secs(20));
        assert_eq!(backoff(base, 2), Duration::from_secs(40));
        assert_eq!(backoff(base, 3), BACKOFF_CAP);
        assert_eq!(backoff(base, 9), BACKOFF_CAP);
    }
}
