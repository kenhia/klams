//! `systemctl is-active` poller used by `klams-monitor` to gather the
//! current state of each watched unit. The result is intentionally
//! coarse — the state-diff layer ([`crate::state`]) decides whether
//! anything is worth posting.

use anyhow::{Context, Result};
use tokio::process::Command;

/// Coarse activation state for a single systemd unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    Active,
    Inactive,
}

/// Shell out to `systemctl is-active <unit>`. Exit code 0 → `Active`,
/// any non-zero exit (including unit-not-found) → `Inactive`. Network
/// or `systemctl`-missing errors propagate.
pub async fn is_active(unit: &str) -> Result<UnitState> {
    let status = Command::new("systemctl")
        .arg("is-active")
        .arg(unit)
        .status()
        .await
        .with_context(|| format!("spawn systemctl is-active {unit}"))?;
    Ok(if status.success() {
        UnitState::Active
    } else {
        UnitState::Inactive
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn is_active_on_bogus_unit_returns_inactive_when_systemctl_present() {
        if which::which("systemctl").is_err() {
            eprintln!("skipping: systemctl not on PATH");
            return;
        }
        let s = is_active("klams-this-unit-does-not-exist.service")
            .await
            .expect("call");
        assert_eq!(s, UnitState::Inactive);
    }
}
