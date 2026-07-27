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
// Sprint 032 (#647): these were `klams_knowledge` / 384 — a collection
// name that has never existed in production and the pre-028 vector
// width. `QdrantStore::connect` CREATES a missing collection, so the
// tool run bare would manufacture an empty `klams_knowledge`, find
// nothing to repair, and exit 0 — a repair tool that reports success
// for doing nothing. The preflight below is the real fix: the default
// can drift again, but the tool can no longer invent its target.
const DEFAULT_COLLECTION: &str = "knowledge_items_v2";
const VECTOR_DIM: u64 = 1024;

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

    // Sprint 032 (#647) — refuse to repair a collection that isn't
    // there. `QdrantStore::connect` creates on absence (deliberately,
    // for the service's cold-start race), which for a *repair* tool is
    // the wrong default entirely: it turns "you pointed me at the wrong
    // collection" into "zero rows needed repair, exit 0".
    ensure_collection_exists(&qdrant_url, &collection).await?;

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

/// Fail loudly when the target collection is absent, listing what *is*
/// there so the operator can fix the pointer in one step.
async fn ensure_collection_exists(qdrant_url: &str, collection: &str) -> anyhow::Result<()> {
    let client = qdrant_client::Qdrant::from_url(qdrant_url)
        .build()
        .with_context(|| format!("qdrant client {qdrant_url}"))?;
    let exists = client
        .collection_exists(collection)
        .await
        .with_context(|| format!("qdrant collection_exists {collection}"))?;
    if exists {
        return Ok(());
    }
    let names = client.list_collections().await.map_or_else(
        |_| "<could not list>".to_string(),
        |r| {
            r.collections
                .into_iter()
                .map(|c| c.name)
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    anyhow::bail!(
        "collection `{collection}` does not exist at {qdrant_url}; \
         refusing to create it. Collections present: {names}. \
         Set KLAMS_QDRANT_COLLECTION to the live one (see `collection` \
         in /etc/klams/klams.toml)."
    )
}
