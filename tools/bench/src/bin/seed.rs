//! `klams-bench seed` — deterministic seeded fixture generator (US5).
//!
//! See `sprints/008-activity-observability/contracts/bench-harness.md`.

use anyhow::{Context, Result};
use klams_bench::{
    generate_facts, generate_knowledge, DEFAULT_FACTS, DEFAULT_KNOWLEDGE, DEFAULT_SEED,
};
use klams_client::{Client, ClientError};
use std::env;
use std::time::Duration;

struct Args {
    seed: u64,
    facts: usize,
    knowledge: usize,
    klams_url: String,
    klams_token: String,
    dry_run: bool,
}

fn parse_args() -> Result<Args> {
    let mut args = Args {
        seed: DEFAULT_SEED,
        facts: DEFAULT_FACTS,
        knowledge: DEFAULT_KNOWLEDGE,
        klams_url: env::var("KLAMS_URL").unwrap_or_else(|_| "http://127.0.0.1:7777".to_string()),
        klams_token: env::var("KLAMS_TOKEN").unwrap_or_default(),
        dry_run: false,
    };
    let mut it = env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seed" => args.seed = parse_u64(&it.next().context("--seed value")?)?,
            "--facts" => args.facts = it.next().context("--facts value")?.parse()?,
            "--knowledge" => args.knowledge = it.next().context("--knowledge value")?.parse()?,
            "--klams-url" => args.klams_url = it.next().context("--klams-url value")?,
            "--klams-token" => args.klams_token = it.next().context("--klams-token value")?,
            "--dry-run" => args.dry_run = true,
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
    if let Err(e) = run().await {
        eprintln!("klams-bench seed: {e:#}");
        std::process::exit(2);
    }
}

async fn run() -> Result<()> {
    let args = parse_args()?;
    eprintln!(
        "klams-bench seed: seed={:#018x} facts={} knowledge={} url={} dry_run={}",
        args.seed, args.facts, args.knowledge, args.klams_url, args.dry_run
    );

    let facts = generate_facts(args.seed, args.facts);
    let knowledge = generate_knowledge(args.seed, args.knowledge);
    eprintln!(
        "klams-bench seed: generated {} facts, {} knowledge items",
        facts.len(),
        knowledge.len()
    );

    if args.dry_run {
        eprintln!("klams-bench seed: --dry-run set; skipping writes");
        return Ok(());
    }

    if args.klams_token.is_empty() {
        anyhow::bail!("--klams-token (or KLAMS_TOKEN env var) required for live writes");
    }
    let client = Client::new(&args.klams_url, args.klams_token.clone())?;

    let progress_every = 500usize;
    let total = facts.len() + knowledge.len();
    let mut done = 0usize;
    let mut fact_err = 0usize;
    let mut know_err = 0usize;

    for (i, req) in facts.iter().enumerate() {
        let res = with_503_retry(|| client.upsert_fact(req)).await;
        if let Err(e) = res {
            fact_err += 1;
            if fact_err <= 5 {
                eprintln!("  fact #{i}: {e}");
            }
        }
        done += 1;
        if done.is_multiple_of(progress_every) {
            eprintln!("  progress: {done}/{total}");
        }
    }
    for (i, req) in knowledge.iter().enumerate() {
        let res = with_503_retry(|| client.index_knowledge(req)).await;
        if let Err(e) = res {
            know_err += 1;
            if know_err <= 5 {
                eprintln!("  knowledge #{i}: {e}");
            }
        }
        done += 1;
        if done.is_multiple_of(progress_every) {
            eprintln!("  progress: {done}/{total}");
        }
    }
    eprintln!("klams-bench seed: complete — fact_err={fact_err}, knowledge_err={know_err}");
    Ok(())
}

async fn with_503_retry<F, Fut, T>(mut f: F) -> Result<T, ClientError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ClientError>>,
{
    let mut delay_ms = 50u64;
    for _ in 0..6 {
        match f().await {
            Ok(v) => return Ok(v),
            Err(ClientError::Api { status, .. }) if status.as_u16() == 503 => {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                delay_ms = (delay_ms * 2).min(2_000);
            }
            Err(e) => return Err(e),
        }
    }
    f().await
}
