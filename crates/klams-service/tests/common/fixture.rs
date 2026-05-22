//! Deterministic fixture generator for sprint 005 integration tests.
//!
//! Builds facts, knowledge chunks, and events from a seeded RNG so
//! every test run produces the same data. Scale presets cover quick
//! smoke runs through summarization/perf-grade volumes.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]

use klams_types::{AppendEvent, FactType, IndexKnowledge, Source, UpsertFact};
use serde_json::json;
use time::OffsetDateTime;
use uuid::Uuid;

const HOSTS: &[&str] = &["alpha", "beta", "kubs0", "kai", "webproxy"];
const CATEGORIES: &[&str] = &["syslog", "pod", "systemd", "ssh", "cron"];
const SERVICES: &[&str] = &["nginx", "kubelet", "containerd", "sshd", "cron"];
const REPOS: &[&str] = &["klams", "docs", "ops"];
const FILE_PREFIXES: &[&str] = &["src/", "docs/", "ops/runbook/"];
const EVENT_PHRASES: &[&str] = &[
    "connection reset by peer",
    "pod scheduled successfully",
    "out of memory killer invoked",
    "user logged in from new device",
    "cron job completed in 12s",
    "tls handshake failure",
    "container image pulled",
    "kernel panic averted",
    "disk usage exceeded threshold",
    "service restarted by systemd",
];
const KNOWLEDGE_TOPICS: &[&str] = &[
    "deployment runbook for production rollouts",
    "incident response checklist for kubernetes outages",
    "guide to debugging high latency on the api gateway",
    "postgres tuning notes for write-heavy workloads",
    "tls certificate rotation procedure",
    "qdrant collection migration playbook",
    "ssh key revocation policy and steps",
    "log aggregation pipeline reference",
    "summarization model evaluation notes",
    "scanner workflow for new package types",
];

#[derive(Debug, Clone, Copy)]
pub struct FixtureScale {
    pub facts: usize,
    pub knowledge: usize,
    pub events: usize,
    pub event_days: i64,
}

impl FixtureScale {
    /// Tiny — fits in seconds, exercises every section. Default for
    /// smoke-style integration tests.
    pub const fn tiny() -> Self {
        Self {
            facts: 30,
            knowledge: 60,
            events: 200,
            event_days: 7,
        }
    }

    /// Small — covers typical hybrid retrieval cases with light load.
    pub const fn small() -> Self {
        Self {
            facts: 200,
            knowledge: 500,
            events: 1_500,
            event_days: 14,
        }
    }

    /// Medium — sprint 005 perf baseline (≈1k/5k/10k).
    pub const fn medium() -> Self {
        Self {
            facts: 1_000,
            knowledge: 5_000,
            events: 10_000,
            event_days: 30,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Fixture {
    pub facts: Vec<UpsertFact>,
    pub knowledge: Vec<IndexKnowledge>,
    pub events: Vec<AppendEvent>,
}

#[must_use]
pub fn generate(scale: FixtureScale) -> Fixture {
    generate_with_seed(scale, 0x0050_05C0_DEC0_FFEE_u64)
}

#[must_use]
pub fn generate_with_seed(scale: FixtureScale, seed: u64) -> Fixture {
    let mut rng = Lcg::new(seed);
    Fixture {
        facts: gen_facts(&mut rng, scale.facts),
        knowledge: gen_knowledge(&mut rng, scale.knowledge),
        events: gen_events(&mut rng, scale.events, scale.event_days),
    }
}

fn gen_facts(rng: &mut Lcg, n: usize) -> Vec<UpsertFact> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let fact_type = match i % 3 {
            0 => FactType::UserFact,
            1 => FactType::TaskFact,
            _ => FactType::EnvFact,
        };
        let host = HOSTS[rng.pick(HOSTS.len())];
        let payload = match fact_type {
            FactType::UserFact => json!({
                "user": format!("user{:04}", i % 200),
                "email": format!("user{:04}@example.com", i % 200),
                "host": host,
            }),
            FactType::TaskFact => json!({
                "task_id": format!("T-{:05}", i),
                "title": format!("task {} on {host}", EVENT_PHRASES[rng.pick(EVENT_PHRASES.len())]),
                "host": host,
            }),
            FactType::EnvFact => json!({
                "host": host,
                "kernel": format!("6.{}.{}-klams", i % 9, i % 50),
                "service": SERVICES[rng.pick(SERVICES.len())],
            }),
        };
        out.push(UpsertFact {
            fact_type,
            payload,
            source: Source::User,
            explicit_id: Some(Uuid::from_u128(
                0x1111_0000_0000_0000_0000_0000_0000_0000 | i as u128,
            )),
            expected_version: Some(0),
        });
    }
    out
}

fn gen_knowledge(rng: &mut Lcg, n: usize) -> Vec<IndexKnowledge> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let repo = REPOS[rng.pick(REPOS.len())];
        let prefix = FILE_PREFIXES[rng.pick(FILE_PREFIXES.len())];
        let topic = KNOWLEDGE_TOPICS[i % KNOWLEDGE_TOPICS.len()];
        let phrase = EVENT_PHRASES[rng.pick(EVENT_PHRASES.len())];
        let text = format!(
            "{topic}. section {i}: this chunk discusses {phrase} \
             along with operational notes specific to the {repo} repository.",
        );
        let content_hash = format!("hash-{i:08x}");
        out.push(IndexKnowledge {
            id: Uuid::from_u128(0x2222_0000_0000_0000_0000_0000_0000_0000 | i as u128),
            text,
            content_hash,
            source: Source::User,
            tags: vec![repo.to_string(), "fixture".to_string()],
            repo: Some(repo.to_string()),
            file: Some(format!("{prefix}note_{i:05}.md")),
            machine: None,
        });
    }
    out
}

fn gen_events(rng: &mut Lcg, n: usize, days: i64) -> Vec<AppendEvent> {
    let mut out = Vec::with_capacity(n);
    let now = OffsetDateTime::now_utc();
    for i in 0..n {
        let host = HOSTS[i % HOSTS.len()]; // round-robin gives stable clusters
        let category = CATEGORIES[(i / HOSTS.len()) % CATEGORIES.len()];
        let day_offset = (i as i64 / (HOSTS.len() * CATEGORIES.len()) as i64) % days;
        let observed_at = now - time::Duration::days(day_offset);
        let service = SERVICES[rng.pick(SERVICES.len())];
        let phrase = EVENT_PHRASES[rng.pick(EVENT_PHRASES.len())];
        let payload = json!({
            "host": host,
            "service": service,
            "event": phrase,
            "observed_at": observed_at.format(&time::format_description::well_known::Rfc3339).unwrap(),
            "seq": i,
        });
        out.push(AppendEvent {
            id: Uuid::from_u128(0x3333_0000_0000_0000_0000_0000_0000_0000 | i as u128),
            task_id: None,
            category: category.to_string(),
            payload,
            source: Source::Controller,
        });
    }
    out
}

/// 64-bit Linear Congruential Generator — deterministic, allocation-free,
/// zero external dependencies. Numbers from Numerical Recipes.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        // Avoid zero state — would lock the sequence.
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn pick(&mut self, modulus: usize) -> usize {
        (self.next_u64() as usize) % modulus.max(1)
    }
}
