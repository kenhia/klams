//! Prometheus metric names recorded by the scanner. Counters are
//! emitted through the `metrics` facade; the binary installs a
//! `metrics-exporter-prometheus` HTTP exporter when `--metrics-listen`
//! is set.

pub const FILES_PROCESSED: &str = "klams_scanner_files_processed_total";
pub const FILES_SKIPPED: &str = "klams_scanner_files_skipped_total";
pub const CHUNKS_INDEXED: &str = "klams_scanner_chunks_indexed_total";
pub const LAST_RUN_TS: &str = "klams_scanner_last_run_timestamp_seconds";

pub fn incr_processed() {
    metrics::counter!(FILES_PROCESSED).increment(1);
}

pub fn incr_skipped(reason: &'static str) {
    metrics::counter!(FILES_SKIPPED, "reason" => reason).increment(1);
}

pub fn add_chunks(n: u64) {
    metrics::counter!(CHUNKS_INDEXED).increment(n);
}

pub fn record_last_run(ts_seconds: f64) {
    metrics::gauge!(LAST_RUN_TS).set(ts_seconds);
}
