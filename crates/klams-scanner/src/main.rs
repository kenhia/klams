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
    about = "Walks the configured roots, indexes changed files, deletes vanished ones."
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
    /// Sprint 035 (#776): no default. A machine-specific default here
    /// ("~/src") silently scanned nothing everywhere else; roots must be
    /// configured explicitly and are validated at startup.
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default = "default_interval")]
    interval_secs: u64,
    #[serde(default = "default_state_dir")]
    state_dir: String,
    /// Sprint 023 (#407): override the host stamped on chunks. Defaults
    /// to the kernel hostname. Set explicitly for the future central
    /// mount-scan mode (#406) where one process scans several hosts.
    #[serde(default)]
    host: Option<String>,
    /// Sprint 027 (#420): the embedding model's input ceiling in tokens.
    /// Must match the service's `[embeddings] max_input_tokens` — the
    /// scanner splits against it so it never publishes a chunk the
    /// service will refuse. Keep the two in step when 028 swaps models.
    #[serde(default = "default_max_input_tokens")]
    max_input_tokens: usize,
}

fn default_interval() -> u64 {
    3600
}
fn default_state_dir() -> String {
    "~/.local/state/klams".into()
}
fn default_max_input_tokens() -> usize {
    klams_types::DEFAULT_MAX_INPUT_TOKENS
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

    let host = cfg.host.clone().unwrap_or_else(klams_scanner::default_host);
    let client = Client::new(&cfg.url, cfg.token.clone()).context("build klams client")?;
    let roots: Vec<PathBuf> = if args.root.is_empty() {
        cfg.roots
            .iter()
            .map(|s| expand(s))
            .collect::<Result<Vec<_>>>()?
    } else {
        args.root.clone()
    };
    // Fail loudly on a misconfigured root instead of warning once per
    // cycle and scanning nothing (sprint 035, #776).
    klams_scanner::validate_roots(&roots)?;

    // Sprint 027 (#420): split against the same ceiling the service
    // enforces, so no chunk is published that the embedder will refuse.
    let embed_limit = klams_types::EmbedLimit::new(cfg.max_input_tokens);

    tracing::info!(
        roots = roots.len(),
        interval_secs = interval.as_secs(),
        once = args.once,
        host = %host,
        max_input_tokens = embed_limit.max_input_tokens(),
        "klams-scanner starting"
    );

    loop {
        for root in &roots {
            if let Err(e) = scan_root(
                &client,
                &cfg.url,
                &cfg.token,
                &host,
                &cursor_path,
                root,
                embed_limit,
            )
            .await
            {
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
        roots: Vec::new(),
        interval_secs: default_interval(),
        state_dir: default_state_dir(),
        host: None,
        max_input_tokens: default_max_input_tokens(),
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
