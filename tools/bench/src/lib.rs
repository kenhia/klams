//! `klams-bench` — sprint 008 perf fixture + harness.
//!
//! Two binaries:
//! - `seed` — deterministic seeded fixture generator (≥ 10k facts +
//!   ≥ 50k knowledge items).
//! - `run` — 100-call `memory_search` latency harness; writes
//!   `sprints/008-activity-observability/perf-baseline.md`.
//!
//! This crate is not a dependency of any shipping binary. It is a
//! workspace member so `just gate` keeps it compilable.

use hdrhistogram::Histogram;
use klams_types::entities::{FactType, Source};
use klams_types::requests::{IndexKnowledgeRequest, UpsertFactRequest};
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha20Rng;
use serde_json::json;
use std::fmt::Write as _;

/// Canonical seed used by `just bench-seed` / `just bench-run`.
pub const DEFAULT_SEED: u64 = 0x0000_C0FF_EE00_0008;
pub const DEFAULT_FACTS: usize = 10_000;
pub const DEFAULT_KNOWLEDGE: usize = 50_000;

const HOSTS: &[&str] = &[
    "kubs0", "kubs1", "kubs2", "kwork", "knas0", "krpi0", "krpi1", "krpi2",
];
const SERVICES: &[&str] = &[
    "widget",
    "postgres",
    "qdrant",
    "tei",
    "klams-service",
    "klams-monitor",
    "kpidash",
    "grafana",
    "prometheus",
];
const OUTCOMES: &[&str] = &["ok", "failed", "skipped", "retried", "degraded"];
const TOPICS: &[&str] = &[
    "deploy",
    "backup",
    "restart",
    "tuning",
    "incident",
    "runbook",
    "scrape",
    "calibration",
    "dissent",
    "schema",
    "pipeline",
    "latency",
];
const FACT_KINDS: &[FactType] = &[FactType::UserFact, FactType::TaskFact, FactType::EnvFact];
const TASK_STATUSES: &[&str] = &["planned", "in_progress", "blocked", "done", "cancelled"];
const SOURCES: &[Source] = &[
    Source::User,
    Source::Controller,
    Source::Task,
    Source::AgentProposal,
];

fn rng_for(seed: u64, stream: u64) -> ChaCha20Rng {
    let mut buf = [0u8; 32];
    buf[..8].copy_from_slice(&seed.to_le_bytes());
    buf[8..16].copy_from_slice(&stream.to_le_bytes());
    ChaCha20Rng::from_seed(buf)
}

fn pick<'a, T: ?Sized>(rng: &mut ChaCha20Rng, slice: &'a [&'a T]) -> &'a T {
    slice.choose(rng).expect("non-empty slice")
}

fn pick_owned<T: Copy>(rng: &mut ChaCha20Rng, slice: &[T]) -> T {
    *slice.choose(rng).expect("non-empty slice")
}

/// Deterministic corpus of fact upsert requests for a given seed.
///
/// Produces payloads that satisfy `klams-core` validators for each
/// `FactType` so the seed binary's writes go through:
/// - `UserFact`: `{ name }`
/// - `TaskFact`: `{ task_id: "ansible-<32hex>", status: enum }`
/// - `EnvFact`:  `{ key: "ENV_STYLE", value: <json>, ... }`
#[must_use]
pub fn generate_facts(seed: u64, n: usize) -> Vec<UpsertFactRequest> {
    let mut rng = rng_for(seed, 0xFAC7);
    (0..n)
        .map(|i| {
            let kind = pick_owned(&mut rng, FACT_KINDS);
            let host = pick(&mut rng, HOSTS);
            let service = pick(&mut rng, SERVICES);
            let topic = pick(&mut rng, TOPICS);
            let outcome = pick(&mut rng, OUTCOMES);
            let payload = match kind {
                FactType::UserFact => json!({
                    "name": format!("bench-user-{i:06}"),
                    "note": format!("{topic} on {host}/{service} → {outcome}"),
                }),
                FactType::TaskFact => {
                    let mut hex = String::with_capacity(32);
                    for _ in 0..32 {
                        let nib: u8 = rng.gen_range(0..16);
                        hex.push(char::from_digit(u32::from(nib), 16).unwrap());
                    }
                    let status = pick_owned(&mut rng, TASK_STATUSES);
                    json!({
                        "task_id": format!("ansible-{hex}"),
                        "status": status,
                        "topic": topic,
                        "subject": format!("{host}/{service}"),
                        "outcome": outcome,
                        "seq": i,
                    })
                }
                FactType::EnvFact => {
                    let key = format!(
                        "BENCH_{}_{}_{:04}",
                        topic.to_uppercase(),
                        service.replace('-', "_").to_uppercase(),
                        i % 10_000
                    );
                    json!({
                        "key": key,
                        "value": json!({
                            "host": host,
                            "outcome": outcome,
                            "topic": topic,
                            "seq": i,
                        }),
                    })
                }
            };
            UpsertFactRequest {
                fact_type: kind,
                payload,
                source: pick_owned(&mut rng, SOURCES),
                explicit_id: None,
                expected_version: Some(0),
            }
        })
        .collect()
}

/// Deterministic corpus of knowledge index requests for a given seed.
#[must_use]
pub fn generate_knowledge(seed: u64, n: usize) -> Vec<IndexKnowledgeRequest> {
    let mut rng = rng_for(seed, 0xC0DE_u64);
    (0..n)
        .map(|i| {
            let host = pick(&mut rng, HOSTS);
            let service = pick(&mut rng, SERVICES);
            let topic = pick(&mut rng, TOPICS);
            let outcome = pick(&mut rng, OUTCOMES);
            let body_words = rng.gen_range(20..=60);
            let mut text = format!("{topic} runbook for {service} on {host}: outcome={outcome}. ");
            for _ in 0..body_words {
                text.push_str(pick(&mut rng, TOPICS));
                text.push(' ');
            }
            let _ = write!(text, "(item #{i:06})");
            IndexKnowledgeRequest {
                text,
                source: pick_owned(&mut rng, SOURCES),
                tags: vec![topic.to_string(), service.to_string()],
                repo: Some("klams".to_string()),
                file: Some(format!("notes/{host}-{service}.md")),
                machine: Some(host.to_string()),
            }
        })
        .collect()
}

/// Stable byte-identical hash of a fact corpus — drives the
/// determinism test (FR-019).
#[must_use]
pub fn canonical_facts_digest(facts: &[UpsertFactRequest]) -> String {
    fnv64(&serde_json::to_string(facts).expect("serializable"))
}

/// Stable byte-identical hash of a knowledge corpus.
#[must_use]
pub fn canonical_knowledge_digest(items: &[IndexKnowledgeRequest]) -> String {
    fnv64(&serde_json::to_string(items).expect("serializable"))
}

fn fnv64(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Render a finalized histogram to the perf-baseline markdown body.
/// The template matches `contracts/bench-harness.md`.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn render_perf_baseline(
    iso8601_utc: &str,
    hostname: &str,
    seed: u64,
    facts_count: usize,
    knowledge_count: usize,
    samples: u64,
    query_count: usize,
    queries: &[String],
    hist: &Histogram<u64>,
) -> String {
    let p50_ms = micros_to_ms(hist.value_at_quantile(0.50));
    let p95_ms = micros_to_ms(hist.value_at_quantile(0.95));
    let p99_ms = micros_to_ms(hist.value_at_quantile(0.99));
    let min_ms = micros_to_ms(hist.min());
    let max_ms = micros_to_ms(hist.max());
    let mean_ms = hist.mean() / 1_000.0;

    let mut s = String::new();
    let _ = writeln!(s, "# Perf baseline — sprint 008");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "> Generated {iso8601_utc} by `just bench-run` on `{hostname}`."
    );
    let _ = writeln!(
        s,
        "> Fixture seed: `{:#018x}` · Store: {} facts, {} knowledge items.",
        seed,
        with_commas(facts_count as u64),
        with_commas(knowledge_count as u64)
    );
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "## `memory_search` latency ({samples} samples across {query_count} queries)"
    );
    let _ = writeln!(s);
    let _ = writeln!(s, "| Metric    |              Value |");
    let _ = writeln!(s, "| --------- | -----------------: |");
    let _ = writeln!(s, "| p50       |       {p50_ms:.1} ms |");
    let _ = writeln!(s, "| p95       |       {p95_ms:.1} ms |");
    let _ = writeln!(s, "| p99       |       {p99_ms:.1} ms |");
    let _ = writeln!(s, "| min / max | {min_ms:.1} ms / {max_ms:.1} ms |");
    let _ = writeln!(s, "| mean      |      {mean_ms:.1} ms |");
    let _ = writeln!(s);
    let _ = writeln!(s, "## Sample queries");
    let _ = writeln!(s);
    for q in queries {
        let _ = writeln!(s, "- `\"{q}\"`");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "## Notes");
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "- Run against a quiescent store; concurrent writes during the run will skew the numbers."
    );
    let _ = writeln!(
        s,
        "- SC-006 threshold (`memory_search` p95 < 1 s) is not enforced by this harness; this file surfaces the measurement. Tuning is gated on user review."
    );
    if facts_count < DEFAULT_FACTS || knowledge_count < DEFAULT_KNOWLEDGE {
        let _ = writeln!(
            s,
            "- **Smoke run**: corpus is below the canonical {}/{} target — rerun `just bench-seed && just bench-run` with the full corpus once kwi work item #26 is fixed.",
            with_commas(DEFAULT_FACTS as u64),
            with_commas(DEFAULT_KNOWLEDGE as u64)
        );
    }
    s
}

fn micros_to_ms(us: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let f = us as f64;
    f / 1_000.0
}

fn with_commas(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// Parse queries.txt content — one query per line, `#` comments and
/// blank lines ignored.
#[must_use]
pub fn parse_queries(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_queries_ignores_comments_and_blanks() {
        let raw = "# header\n\nwidget deploy\n  \n# another\nkubs0 backup\n";
        assert_eq!(
            parse_queries(raw),
            vec!["widget deploy".to_string(), "kubs0 backup".to_string()]
        );
    }

    #[test]
    fn render_perf_baseline_contains_required_sections() {
        let mut h: Histogram<u64> = Histogram::new_with_bounds(1, 60_000_000, 3).unwrap();
        for v in [1_000u64, 2_000, 5_000, 10_000, 20_000, 50_000] {
            h.record(v).unwrap();
        }
        let md = render_perf_baseline(
            "2026-05-26T00:00:00Z",
            "kubs0",
            DEFAULT_SEED,
            10_247,
            50_138,
            6,
            10,
            &["q1".into(), "q2".into()],
            &h,
        );
        assert!(md.contains("# Perf baseline — sprint 008"));
        assert!(md.contains("## `memory_search` latency"));
        assert!(md.contains("| p50       |"));
        assert!(md.contains("10,247 facts"));
        assert!(md.contains("`\"q1\"`"));
        assert!(md.contains("SC-006 threshold"));
    }
}
