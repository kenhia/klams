//! Sprint 009 T029 — `reattribute-system` admin CLI.
//!
//! Contract: [`sprints/009-stability-attribution/contracts/reattribution-cli.md`].

use anyhow::Context;
use clap::Parser;
use klams_store::{
    repair::{reattribute_system_owned, RepairMode},
    PostgresStore, QdrantStore,
};
use std::process::ExitCode;

/// Exactly one of `--dry-run` / `--apply` is required.
#[derive(Debug, Parser)]
#[command(name = "reattribute-system", about)]
struct Cli {
    /// Count without writing.
    #[arg(long, conflicts_with = "apply")]
    dry_run: bool,
    /// Apply the reassignments.
    #[arg(long)]
    apply: bool,
    /// Optional path to also write the JSON report to.
    #[arg(long)]
    report_out: Option<String>,
}

const DEFAULT_DB: &str = "postgres://klams:klams@127.0.0.1:5432/klams";
const DEFAULT_QDRANT: &str = "http://127.0.0.1:6334";
const DEFAULT_COLLECTION: &str = "klams_knowledge";
const VECTOR_DIM: u64 = 384;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cli = Cli::parse();
    let mode = match (cli.dry_run, cli.apply) {
        (true, false) => RepairMode::DryRun,
        (false, true) => RepairMode::Apply,
        _ => {
            eprintln!("error: exactly one of --dry-run / --apply is required");
            return ExitCode::from(2);
        }
    };
    match run(mode, cli.report_out.as_deref()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("reattribute-system: {e:#}");
            ExitCode::from(1)
        }
    }
}

async fn run(mode: RepairMode, report_out: Option<&str>) -> anyhow::Result<()> {
    let db_url = std::env::var("KLAMS_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DB.to_string());
    let qdrant_url =
        std::env::var("KLAMS_QDRANT_URL").unwrap_or_else(|_| DEFAULT_QDRANT.to_string());
    let collection =
        std::env::var("KLAMS_QDRANT_COLLECTION").unwrap_or_else(|_| DEFAULT_COLLECTION.to_string());

    let postgres = PostgresStore::connect(&db_url, 4)
        .await
        .with_context(|| format!("connect Postgres {db_url}"))?;
    let qdrant = QdrantStore::connect(&qdrant_url, &collection, VECTOR_DIM)
        .await
        .with_context(|| format!("connect Qdrant {qdrant_url} (collection {collection})"))?;

    let report = reattribute_system_owned(&postgres, &qdrant, mode)
        .await
        .context("repair failed")?;
    let json = serde_json::to_string_pretty(&report)?;
    println!("{json}");
    if let Some(path) = report_out {
        std::fs::write(path, &json).with_context(|| format!("write report-out {path}"))?;
    }
    Ok(())
}
