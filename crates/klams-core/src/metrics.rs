//! Named Prometheus metrics for klams.
//!
//! All counters/histograms are described once on import so they show
//! up in `/metrics` even before the first observation. Handlers and
//! the worker pool call into the constants below to keep label
//! cardinality fixed.

use metrics::{counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram};
use std::time::Instant;

pub const QUEUE_DEPTH: &str = "klams_queue_depth";
pub const QUEUE_CAPACITY: &str = "klams_queue_capacity";
pub const WORKERS_ACTIVE: &str = "klams_workers_active";
pub const WRITES_ACCEPTED: &str = "klams_writes_accepted_total";
pub const WRITES_FAILED: &str = "klams_writes_failed_total";
pub const WRITE_LATENCY: &str = "klams_write_latency_seconds";
pub const SEARCH_LATENCY: &str = "klams_search_latency_seconds";
pub const EMBEDDING_LATENCY: &str = "klams_embedding_latency_seconds";

/// Register descriptions with the global recorder. Safe to call
/// repeatedly; the metrics crate dedupes.
pub fn describe() {
    describe_gauge!(QUEUE_DEPTH, "Current depth of the write queue");
    describe_gauge!(QUEUE_CAPACITY, "Configured capacity of the write queue");
    describe_gauge!(WORKERS_ACTIVE, "Number of active worker tasks");
    describe_counter!(WRITES_ACCEPTED, "Total writes accepted onto the queue");
    describe_counter!(WRITES_FAILED, "Total writes that failed at the store");
    describe_histogram!(WRITE_LATENCY, "End-to-end write latency in seconds");
    describe_histogram!(SEARCH_LATENCY, "End-to-end search latency in seconds");
    describe_histogram!(EMBEDDING_LATENCY, "Embedding call latency in seconds");
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
