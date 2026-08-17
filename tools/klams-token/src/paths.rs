//! Finding the config this tool is meant to edit.
//!
//! Mirrors `klams-service`'s own `resolve_config_path` so the CLI and
//! the service agree on which file is live, with one addition: the
//! service is normally started by `deploy/klams-service.service`, which
//! sets `KLAMS_CONFIG=/etc/klams/klams.toml` in the *unit*, not in the
//! operator's shell. Without that fallback a bare `sudo klams-token
//! list` on a systemd host would report "no config found" while the
//! service was happily running against one.
//!
//! Every path in the chain is a default this repo already ships. None
//! is invented here, and none is host-specific — the error names every
//! path it tried rather than guessing (AGENTS.md's portability line).

use anyhow::{bail, Result};
use std::path::PathBuf;

/// `$KLAMS_ROOT`-style layout, the same constant `klams-service` uses.
pub const ROOT_CONFIG: &str = "/ai/klams/config/klams.toml";
/// Where `deploy/install-systemd.sh` puts it and the unit points at.
pub const SYSTEMD_CONFIG: &str = "/etc/klams/klams.toml";

/// Resolve the config path: explicit `--config`, then `$KLAMS_CONFIG`,
/// then the two shipped locations, then XDG.
///
/// # Errors
/// If nothing exists, with every candidate named.
pub fn resolve(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(p) = std::env::var("KLAMS_CONFIG") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let candidates = candidates();
    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }
    bail!(
        "no klams config found — tried {}\nset KLAMS_CONFIG or pass --config",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The shipped locations, in the order the service would find them.
#[must_use]
pub fn candidates() -> Vec<PathBuf> {
    vec![
        PathBuf::from(ROOT_CONFIG),
        PathBuf::from(SYSTEMD_CONFIG),
        xdg_config(),
    ]
}

fn xdg_config() -> PathBuf {
    // Per the XDG spec an empty XDG_CONFIG_HOME is treated as unset.
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map_or_else(
            || PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"),
            PathBuf::from,
        );
    base.join("klams").join("klams.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_wins_over_everything() {
        let p = resolve(Some(PathBuf::from("/tmp/somewhere.toml"))).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/somewhere.toml"));
    }

    /// The systemd location is the one the service actually runs
    /// against on a deployed host, and it is only reachable because
    /// the unit exports `KLAMS_CONFIG` — which the operator's shell
    /// does not. Dropping it from the chain would make a bare
    /// `sudo klams-token list` claim there is no config.
    #[test]
    fn the_chain_covers_both_shipped_locations() {
        let c = candidates();
        assert!(c.contains(&PathBuf::from(ROOT_CONFIG)));
        assert!(c.contains(&PathBuf::from(SYSTEMD_CONFIG)));
        assert_eq!(c.len(), 3, "the third candidate is the XDG fallback");
    }
}
