//! `klams-scanner` binary entry point (sprint 003 T038/T039).
//!
//! Walks each configured root, diffs against the local sqlite cursor,
//! chunks changed files, posts chunks to `/memory/knowledge/index`,
//! and deletes vanished files via `/memory/knowledge/delete`.

use anyhow::{Context, Result};
use clap::Parser;
use klams_client::Client;
use klams_scanner::{metrics as sm, scan_root};
use metrics_exporter_prometheus::PrometheusBuilder;
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Walks ~/src and ~/obsidian, indexes changed files, deletes vanished ones."
)]
struct Args {
    #[arg(long, env = "KLAMS_CONFIG")]
    config: Option<PathBuf>,
    /// Scan once and exit.
    #[arg(long)]
    once: bool,
    /// Ad-hoc root override (skips config). Repeatable.
    #[arg(long)]
    root: Vec<PathBuf>,
    /// Override the configured interval.
    #[arg(long)]
    interval_secs: Option<u64>,
    /// `host:port` for the Prometheus metrics endpoint.
    #[arg(long)]
    metrics_listen: Option<SocketAddr>,
    /// klams URL (overrides config).
    #[arg(long, env = "KLAMS_URL")]
    url: Option<String>,
    /// klams bearer token (overrides config).
    #[arg(long, env = "KLAMS_TOKEN")]
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Config {
    url: String,
    token: String,
    #[serde(default = "default_roots")]
    roots: Vec<String>,
    #[serde(default = "default_interval")]
    interval_secs: u64,
    #[serde(default = "default_state_dir")]
    state_dir: String,
}

fn default_roots() -> Vec<String> {
    vec!["~/src".into(), "~/obsidian".into()]
}
fn default_interval() -> u64 {
    3600
}
fn default_state_dir() -> String {
    "~/.local/state/klams".into()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();

    let cfg = load_config(&args)?;
    let interval = Duration::from_secs(args.interval_secs.unwrap_or(cfg.interval_secs));
    let state_dir = expand(&cfg.state_dir)?;
    std::fs::create_dir_all(&state_dir)
        .with_context(|| format!("create state dir {}", state_dir.display()))?;
    let cursor_path = state_dir.join("scanner.sqlite");

    if let Some(addr) = args.metrics_listen {
        PrometheusBuilder::new()
            .with_http_listener(addr)
            .install()
            .context("install Prometheus exporter")?;
        tracing::info!(%addr, "metrics endpoint listening");
    }

    let client = Client::new(&cfg.url, cfg.token.clone()).context("build klams client")?;
    let roots: Vec<PathBuf> = if args.root.is_empty() {
        cfg.roots
            .iter()
            .map(|s| expand(s))
            .collect::<Result<Vec<_>>>()?
    } else {
        args.root.clone()
    };

    tracing::info!(
        roots = roots.len(),
        interval_secs = interval.as_secs(),
        once = args.once,
        "klams-scanner starting"
    );

    loop {
        for root in &roots {
            if let Err(e) = scan_root(&client, &cfg.url, &cfg.token, &cursor_path, root).await {
                tracing::warn!(root = %root.display(), error = %e, "scan failed");
            }
        }
        sm::record_last_run(now_seconds_f64());
        if args.once {
            break;
        }
        tokio::time::sleep(interval).await;
    }
    Ok(())
}

fn load_config(args: &Args) -> Result<Config> {
    if let Some(path) = &args.config {
        let body = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        return toml::from_str(&body).context("parse config TOML");
    }
    let url = args.url.clone().context("--url or --config required")?;
    let token = args.token.clone().context("--token or --config required")?;
    Ok(Config {
        url,
        token,
        roots: default_roots(),
        interval_secs: default_interval(),
        state_dir: default_state_dir(),
    })
}

fn expand(s: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(
        shellexpand::full(s).context("expand path")?.into_owned(),
    ))
}

fn now_seconds_f64() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or_default()
}
