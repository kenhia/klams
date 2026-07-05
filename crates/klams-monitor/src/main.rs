//! `klams-monitor` binary entry point (sprint 003 T029).
//!
//! Loads a TOML config, polls each unit on a fixed interval, and posts
//! `Service` events to a running klams API whenever the in-memory
//! state diff produces one. Use `--once` for one-shot polls under
//! `cron`/`systemd.timer`; otherwise it loops forever.
//!
//! ## Known limitation: the monitor cannot record its own sink's `Down` (#55)
//!
//! Events are published to `klams-service` itself. When `klams-service` is the
//! monitored unit and it goes down, the `Down` publish fails and is dropped —
//! there is no local buffer or retry, and the state cache advances regardless
//! (see the `publish failed` branch below), so the very outage the monitor
//! exists to observe is the one it cannot post.
//!
//! This is documented rather than fixed (a durable spool was deliberately out of
//! scope for sprint 012). It is tolerable because a `klams-service` outage is
//! still **reconstructable from the gap**: when the service recovers, the next
//! poll diffs `Down -> Up` and successfully posts the recovery `Up`. The window
//! between the last good `Up` (e.g. the cold-start event) and that recovery `Up`
//! brackets the downtime. Other monitored units are unaffected — their events go
//! to a sink independent of them, so their `Down` records normally.

use anyhow::{Context, Result};
use clap::Parser;
use klams_client::Client;
use klams_monitor::{
    poll::is_active,
    publish::publish,
    state::{apply, diff, PollObservation, PreviousState},
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Polls systemd units and posts state changes to klams"
)]
struct Args {
    /// Path to the TOML config file.
    #[arg(long, env = "KLAMS_CONFIG")]
    config: PathBuf,
    /// Poll once and exit (useful for cron / one-shot systemd timers).
    #[arg(long)]
    once: bool,
    /// Override the config's `interval_secs`.
    #[arg(long)]
    interval_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Config {
    url: String,
    token: String,
    units: Vec<String>,
    #[serde(default = "default_interval")]
    interval_secs: u64,
    #[serde(default = "default_host")]
    host: String,
    /// Optional kpidash dashboard reporting (sprint 010 / T024). Absent in a
    /// fresh clone, so nothing connects to Redis unless the operator opts in.
    #[cfg(feature = "kpidash")]
    #[serde(default)]
    kpidash: Option<klams_monitor::kpidash::KpidashConfig>,
}

fn default_interval() -> u64 {
    15
}

/// Best-effort host identity stamped on every `Service` event.
///
/// systemd does not export `$HOSTNAME` to service units, so the old
/// `std::env::var("HOSTNAME")` fallback always yielded `"unknown"` and stripped
/// host attribution from every event (#56). Read the kernel's live hostname from
/// procfs instead — identical to the `gethostname(2)` syscall on Linux, with no
/// unit/config dependency and no extra crate. `$HOSTNAME` (also set via
/// `Environment=HOSTNAME=%H` in the unit) is kept as a fallback, then `unknown`.
fn default_host() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let body = std::fs::read_to_string(&args.config)
        .with_context(|| format!("read config {}", args.config.display()))?;
    let cfg: Config = toml::from_str(&body).context("parse config TOML")?;
    let interval = Duration::from_secs(args.interval_secs.unwrap_or(cfg.interval_secs));
    let client = Client::new(&cfg.url, cfg.token.clone()).context("build klams client")?;
    let mut prev: HashMap<String, PreviousState> = HashMap::new();
    tracing::info!(
        units = cfg.units.len(),
        interval_secs = interval.as_secs(),
        once = args.once,
        "klams-monitor starting"
    );
    // kpidash dashboard reporter (sprint 010 / T024): polls healthz and writes
    // a status card to Redis, replacing the legacy python looper. Inert unless
    // a `[kpidash]` config section is present.
    #[cfg(feature = "kpidash")]
    if let Some(reporter) = cfg
        .kpidash
        .clone()
        .map(|kc| klams_monitor::kpidash::Reporter::new(kc, &cfg.url))
    {
        tracing::info!(
            interval_secs = reporter.interval().as_secs(),
            "kpidash reporter enabled"
        );
        if args.once {
            let mut reporter = reporter;
            reporter.report_once().await;
        } else {
            tokio::spawn(reporter.run());
        }
    }
    loop {
        for unit in &cfg.units {
            match is_active(unit).await {
                Ok(state) => {
                    let entry = prev.entry(unit.clone()).or_default();
                    let obs = PollObservation {
                        service: unit,
                        host: &cfg.host,
                        current_state: state,
                        current_version: None,
                    };
                    if let Some(payload) = diff(entry, &obs) {
                        tracing::info!(unit = %unit, ?payload, "state change");
                        if let Err(e) = publish(&client, &payload).await {
                            // #55: no local buffer/retry — a failed publish is
                            // dropped. When `unit` is the sink (klams-service)
                            // itself, its `Down` is unrecordable here; it stays
                            // reconstructable from the gap to the recovery `Up`.
                            // See the module-level "Known limitation" note.
                            tracing::warn!(unit = %unit, error = %e, "publish failed");
                        }
                    }
                    // Advances even after a failed publish (see #55 note above):
                    // intentional so a persistently-down sink doesn't re-emit the
                    // same dropped `Down` every tick.
                    apply(entry, &obs);
                }
                Err(e) => tracing::warn!(unit = %unit, error = %e, "poll failed"),
            }
        }
        if args.once {
            break;
        }
        tokio::time::sleep(interval).await;
    }
    Ok(())
}
