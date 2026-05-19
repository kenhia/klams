//! `klams-monitor` binary entry point (sprint 003 T029).
//!
//! Loads a TOML config, polls each unit on a fixed interval, and posts
//! `Service` events to a running klams API whenever the in-memory
//! state diff produces one. Use `--once` for one-shot polls under
//! `cron`/`systemd.timer`; otherwise it loops forever.

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
    #[arg(long, env = "KLAMS_MONITOR_CONFIG")]
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
}

fn default_interval() -> u64 {
    15
}

fn default_host() -> String {
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
                            tracing::warn!(unit = %unit, error = %e, "publish failed");
                        }
                    }
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
