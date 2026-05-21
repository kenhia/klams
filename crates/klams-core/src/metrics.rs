//! Named Prometheus metrics for klams.
//!
//! All counters/histograms are described once on import so they show
//! up in `/metrics` even before the first observation. Handlers and
//! the worker pool call into the constants below to keep label
//! cardinality fixed.

use klams_types::{Source, WritePath};
use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use std::time::Instant;

pub const QUEUE_DEPTH: &str = "klams_queue_depth";
pub const QUEUE_CAPACITY: &str = "klams_queue_capacity";
pub const WORKERS_ACTIVE: &str = "klams_workers_active";
pub const WRITES_ACCEPTED: &str = "klams_writes_accepted_total";
pub const WRITES_FAILED: &str = "klams_writes_failed_total";
pub const WRITES_TOTAL: &str = "klams_writes_total";
pub const WRITE_LATENCY: &str = "klams_write_latency_seconds";
pub const SEARCH_LATENCY: &str = "klams_search_latency_seconds";
pub const EMBEDDING_LATENCY: &str = "klams_embedding_latency_seconds";
pub const VALIDATION_REJECTIONS: &str = "klams_validation_rejections_total";
pub const DISSENTS_TOTAL: &str = "klams_dissents_total";
pub const VERSION_CONFLICTS: &str = "klams_version_conflicts_total";
pub const LAST_USED_BUMPS_DROPPED: &str = "klams_last_used_bumps_dropped_total";
pub const DECAY_RUNS: &str = "klams_decay_runs_total";
pub const DECAY_FACTS_UPDATED: &str = "klams_decay_facts_updated_total";

// Sprint 005 (FR-014): `/memory/context` surface metrics.
pub const CONTEXT_REQUEST_LATENCY: &str = "klams_context_request_seconds";
pub const CONTEXT_SECTION_ITEMS: &str = "klams_context_section_items_total";
pub const HYBRID_SOURCE_CONTRIBUTION: &str = "klams_hybrid_source_contribution_total";
pub const SUMMARIZATION_RUNS: &str = "klams_summarization_runs_total";
pub const SUMMARIZATION_LAG: &str = "klams_summarization_lag_seconds";
pub const DECAY_CONFIG_RELOADS: &str = "klams_decay_config_reloads_total";

/// Register descriptions with the global recorder. Safe to call
/// repeatedly; the metrics crate dedupes.
pub fn describe() {
    describe_gauge!(QUEUE_DEPTH, "Current depth of the write queue");
    describe_gauge!(QUEUE_CAPACITY, "Configured capacity of the write queue");
    describe_gauge!(WORKERS_ACTIVE, "Number of active worker tasks");
    describe_counter!(WRITES_ACCEPTED, "Total writes accepted onto the queue");
    describe_counter!(WRITES_FAILED, "Total writes that failed at the store");
    describe_counter!(
        WRITES_TOTAL,
        "Sprint 003 (FR-017): every write tagged by type, source, and routing path"
    );
    describe_histogram!(WRITE_LATENCY, "End-to-end write latency in seconds");
    describe_histogram!(SEARCH_LATENCY, "End-to-end search latency in seconds");
    describe_histogram!(EMBEDDING_LATENCY, "Embedding call latency in seconds");
    describe_counter!(
        VALIDATION_REJECTIONS,
        "Validation rejections at the API ingress (sprint 002 US1)"
    );
    describe_counter!(
        DISSENTS_TOTAL,
        "Dissent lifecycle events by outcome (sprint 002 US2)"
    );
    describe_counter!(
        VERSION_CONFLICTS,
        "Optimistic-version conflicts on canonical writes (sprint 002 US2)"
    );
    describe_counter!(
        LAST_USED_BUMPS_DROPPED,
        "`last_used_at` bumps dropped because the coalesce channel was full"
    );
    describe_counter!(DECAY_RUNS, "Background decay-task passes (sprint 002 US3)");
    describe_counter!(
        DECAY_FACTS_UPDATED,
        "Facts updated by the decay task (sprint 002 US3)"
    );
    describe_histogram!(
        CONTEXT_REQUEST_LATENCY,
        "End-to-end /memory/context latency in seconds (sprint 005 FR-014)"
    );
    describe_counter!(
        CONTEXT_SECTION_ITEMS,
        "Items returned per /memory/context section, labelled by section name (sprint 005 FR-014)"
    );
    describe_counter!(
        HYBRID_SOURCE_CONTRIBUTION,
        "Hybrid retrieval per-source row contribution before fusion (sprint 005 FR-014)"
    );
    describe_counter!(
        SUMMARIZATION_RUNS,
        "Summarization task runs by kind and outcome (sprint 005 FR-014)"
    );
    describe_gauge!(
        SUMMARIZATION_LAG,
        "Seconds since the last successful summarization task run (sprint 005 FR-014)"
    );
    describe_counter!(
        DECAY_CONFIG_RELOADS,
        "Decay config reload events at startup (sprint 005 FR-014)"
    );
}

/// Update queue gauges; call from the API layer after a successful
/// or rejected enqueue.
#[allow(clippy::cast_precision_loss)] // gauge values are observational; usize > f64 mantissa is impossible at MVP scale
pub fn record_queue(depth: usize, capacity: usize, workers: usize) {
    gauge!(QUEUE_DEPTH).set(depth as f64);
    gauge!(QUEUE_CAPACITY).set(capacity as f64);
    gauge!(WORKERS_ACTIVE).set(workers as f64);
}

pub fn incr_writes_accepted(kind: &'static str) {
    counter!(WRITES_ACCEPTED, "type" => kind).increment(1);
}

pub fn incr_writes_failed(kind: &'static str, reason: &'static str) {
    counter!(WRITES_FAILED, "type" => kind, "reason" => reason).increment(1);
}

/// Sprint 003 (FR-017): one tick per durable write attempt, labelled
/// with the API endpoint type, the request `source`, and the routing
/// `path` (canonical/dissent). Call this exactly once at the
/// success/divert site of every write endpoint.
pub fn incr_writes_total(kind: &'static str, source: Source, path: WritePath) {
    counter!(
        WRITES_TOTAL,
        "type" => kind,
        "source" => source_label(source),
        "path" => path.as_str(),
    )
    .increment(1);
}

fn source_label(s: Source) -> &'static str {
    match s {
        Source::User => "user",
        Source::Controller => "controller",
        Source::Task => "task",
        Source::AgentProposal => "agent_proposal",
    }
}

pub fn incr_validation_rejection(rule: &'static str) {
    counter!(VALIDATION_REJECTIONS, "rule" => rule).increment(1);
}

/// Increment the validation rejections counter for each rule id in
/// `rules`. Unknown rule ids are bucketed under `"other"` so counter
/// cardinality stays bounded.
pub fn incr_validation_rejections_owned(rules: &[String]) {
    for r in rules {
        let label = canonical_validation_rule(r);
        counter!(VALIDATION_REJECTIONS, "rule" => label).increment(1);
    }
}

const KNOWN_VALIDATION_RULES: &[&str] = &[
    "required",
    "type",
    "enum",
    "length",
    "shape",
    "uuid",
    "rfc3339",
    "email_shape",
    "hostname_shape",
    "timestamp_range",
    "numeric_range",
    "expected_version_required",
];

fn canonical_validation_rule(rule: &str) -> &'static str {
    for k in KNOWN_VALIDATION_RULES {
        if *k == rule {
            return k;
        }
    }
    "other"
}

pub fn incr_dissent_outcome(outcome: &'static str) {
    counter!(DISSENTS_TOTAL, "outcome" => outcome).increment(1);
}

pub fn incr_version_conflict() {
    counter!(VERSION_CONFLICTS).increment(1);
}

pub fn incr_last_used_bumps_dropped() {
    counter!(LAST_USED_BUMPS_DROPPED).increment(1);
}

pub fn incr_decay_run() {
    counter!(DECAY_RUNS).increment(1);
}

pub fn incr_decay_facts_updated(n: u64) {
    counter!(DECAY_FACTS_UPDATED).increment(n);
}

/// Sprint 005 (FR-014): one tick per /memory/context section
/// returned, with the section name as the label. Labels are a
/// fixed enum (`facts`/`knowledge`/`events`) so cardinality stays
/// bounded.
pub fn incr_context_section_items(section: &'static str, count: u64) {
    counter!(CONTEXT_SECTION_ITEMS, "section" => section).increment(count);
}

/// Sprint 005 (FR-014): per-source contribution counter for the
/// hybrid retrieval path. Labels: `source` ∈
/// {`vector`,`fts`,`metadata`}.
pub fn incr_hybrid_source_contribution(source: &'static str, count: u64) {
    counter!(HYBRID_SOURCE_CONTRIBUTION, "source" => source).increment(count);
}

/// RAII guard that records elapsed seconds into a histogram on drop.
pub struct LatencyGuard {
    name: &'static str,
    label: Option<(&'static str, &'static str)>,
    start: Instant,
}

impl LatencyGuard {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            label: None,
            start: Instant::now(),
        }
    }

    pub fn with_type(name: &'static str, kind: &'static str) -> Self {
        Self {
            name,
            label: Some(("type", kind)),
            start: Instant::now(),
        }
    }
}

impl std::fmt::Debug for LatencyGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatencyGuard")
            .field("name", &self.name)
            .field("label", &self.label)
            .field("start", &self.start)
            .finish()
    }
}

impl Drop for LatencyGuard {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_secs_f64();
        match self.label {
            Some((k, v)) => histogram!(self.name, k => v).record(elapsed),
            None => histogram!(self.name).record(elapsed),
        }
    }
}
