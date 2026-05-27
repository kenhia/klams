//! `klams-bench run` — 100-call `memory_search` latency harness (US5).
//!
//! Per FR-022 this binary always exits 0; it surfaces the measurement
//! to `perf-baseline.md` but never gates `just gate`.

use anyhow::{Context, Result};
use hdrhistogram::Histogram;
use klams_bench::{
    parse_queries, render_perf_baseline, DEFAULT_FACTS, DEFAULT_KNOWLEDGE, DEFAULT_SEED,
};
use klams_client::Client;
use klams_types::requests::SearchRequest;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use time::OffsetDateTime;

struct Args {
    klams_url: String,
    klams_token: String,
    queries_path: PathBuf,
    repeats: usize,
    output: PathBuf,
    facts: usize,
    knowledge: usize,
    seed: u64,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        klams_url: env::var("KLAMS_URL").unwrap_or_else(|_| "http://127.0.0.1:7777".to_string()),
        klams_token: env::var("KLAMS_TOKEN").unwrap_or_default(),
        queries_path: PathBuf::from("tools/bench/queries.txt"),
        repeats: 10,
        output: PathBuf::from("specs/008-activity-observability/perf-baseline.md"),
        facts: DEFAULT_FACTS,
        knowledge: DEFAULT_KNOWLEDGE,
        seed: DEFAULT_SEED,
    };
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--klams-url" => args.klams_url = it.next().context("--klams-url value")?,
            "--klams-token" => args.klams_token = it.next().context("--klams-token value")?,
            "--queries" => args.queries_path = PathBuf::from(it.next().context("--queries value")?),
            "--repeats" => args.repeats = it.next().context("--repeats value")?.parse()?,
            "--output" => args.output = PathBuf::from(it.next().context("--output value")?),
            "--facts" => args.facts = it.next().context("--facts value")?.parse()?,
            "--knowledge" => args.knowledge = it.next().context("--knowledge value")?.parse()?,
            "--seed" => args.seed = parse_u64(&it.next().context("--seed value")?)?,
            other => anyhow::bail!("unknown arg: {other}"),
        }
    }
    Ok(args)
}

fn parse_u64(s: &str) -> Result<u64> {
    if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Ok(u64::from_str_radix(&h.replace('_', ""), 16)?)
    } else {
        Ok(s.replace('_', "").parse()?)
    }
}

#[tokio::main]
async fn main() {
    // FR-022: always exit 0.
    if let Err(e) = run().await {
        eprintln!("klams-bench run: {e:#}");
    }
}

async fn run() -> Result<()> {
    let args = parse_args()?;

    let raw = fs::read_to_string(&args.queries_path)
        .with_context(|| format!("read queries from {}", args.queries_path.display()))?;
    let queries = parse_queries(&raw);
    if queries.is_empty() {
        anyhow::bail!("no queries found in {}", args.queries_path.display());
    }

    if args.klams_token.is_empty() {
        anyhow::bail!("--klams-token (or KLAMS_TOKEN env var) required");
    }
    let client = Client::new(&args.klams_url, args.klams_token.clone())?;

    let mut hist: Histogram<u64> = Histogram::new_with_bounds(1, 60_000_000, 3)?;
    let total_samples = queries.len() * args.repeats;
    eprintln!(
        "klams-bench run: {} queries × {} repeats = {} samples",
        queries.len(),
        args.repeats,
        total_samples
    );

    let mut errors = 0usize;
    for (qi, q) in queries.iter().enumerate() {
        for r in 0..args.repeats {
            let req = SearchRequest {
                query: q.clone(),
                types: None,
                filters: None,
                top_k: 10,
            };
            let t0 = Instant::now();
            match client.search(&req).await {
                Ok(_) => {
                    let us = u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX);
                    let us = us.clamp(1, 60_000_000);
                    let _ = hist.record(us);
                }
                Err(e) => {
                    errors += 1;
                    if errors <= 5 {
                        eprintln!("  q{qi}.{r} error: {e}");
                    }
                }
            }
        }
    }

    if hist.is_empty() {
        anyhow::bail!("no successful samples recorded ({errors} errors)");
    }

    let now = OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".into());
    let hostname = hostname_or_unknown();
    let md = render_perf_baseline(
        &now,
        &hostname,
        args.seed,
        args.facts,
        args.knowledge,
        hist.len(),
        queries.len(),
        &queries,
        &hist,
    );
    fs::write(&args.output, md).with_context(|| format!("write {}", args.output.display()))?;
    eprintln!(
        "klams-bench run: wrote {} ({} samples, {} errors)",
        args.output.display(),
        hist.len(),
        errors
    );
    Ok(())
}

fn hostname_or_unknown() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into())
}
