//! Background summarization task: event clusters + stale knowledge clusters.
//!
//! Sprint 005 (Phase 4) — T036. The task wakes on a fixed interval,
//! probes Ollama (if `llm_fallback = true`), then writes one
//! `EventSummary` per detected `(host, category, day_bucket)`
//! cluster. Cycles never lap — a `tokio::sync::Mutex<()>` guards
//! the run loop.
//!
//! For Phase 4 we only emit event summaries; knowledge digests
//! follow when the Qdrant `kind = "digest"` payload is wired up
//! (T038 — tracked for follow-up).

pub mod extractive;
pub mod llm;

use crate::metrics as m;
use extractive::{event_headline, EventCluster};
use klams_store::SummaryStore;
use klams_types::{EventSummary, SummaryMechanism};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use time::{Date, OffsetDateTime};
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct SummarizationConfig {
    pub enabled: bool,
    pub event_cluster_min: u32,
    pub llm_fallback: bool,
    pub task_interval: Duration,
    pub ollama_url: String,
    pub ollama_model: String,
}

/// Cluster key emitted by a source-supplied iterator of `EventRecord`s.
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub id: Uuid,
    pub host: String,
    pub category: String,
    pub sub_category: String,
    pub at: OffsetDateTime,
}

#[async_trait::async_trait]
pub trait EventSource: Send + Sync + 'static {
    /// Return all events since the cutoff that the task should
    /// consider for clustering. Implementations should be paged
    /// when the corpus is large; for Phase 4 we expect a few
    /// thousand events per cycle.
    async fn list_recent(
        &self,
        cutoff: OffsetDateTime,
    ) -> Result<Vec<EventRecord>, klams_store::StoreError>;
}

/// Single-run pure clustering pass exposed for testing.
#[must_use]
pub fn cluster_events(records: &[EventRecord]) -> Vec<EventSummary> {
    type Key = (String, String, Date);
    let mut buckets: HashMap<Key, Vec<&EventRecord>> = HashMap::new();
    for r in records {
        let key = (r.host.clone(), r.category.clone(), r.at.date());
        buckets.entry(key).or_default().push(r);
    }
    let mut out = Vec::with_capacity(buckets.len());
    for ((host, category, day_bucket), items) in buckets {
        let mut sub_counts: HashMap<&str, u32> = HashMap::new();
        for r in &items {
            *sub_counts.entry(r.sub_category.as_str()).or_insert(0) += 1;
        }
        let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
        let earliest = items.iter().map(|r| r.at).min();
        let latest = items.iter().map(|r| r.at).max();
        let earliest_iso = earliest.map(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });
        let latest_iso = latest.map(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });
        let cluster = EventCluster {
            host: host.as_str(),
            category: category.as_str(),
            sub_counts,
            total,
            earliest_iso: earliest_iso.as_deref(),
            latest_iso: latest_iso.as_deref(),
        };
        let summary_text = event_headline(&cluster);
        let mut source_ids: Vec<Uuid> = items.iter().map(|r| r.id).collect();
        source_ids.sort();
        out.push(EventSummary {
            id: Uuid::now_v7(),
            host,
            category,
            day_bucket,
            source_count: total,
            source_ids,
            summary_text,
            mechanism: SummaryMechanism::Extractive,
            generated_at: OffsetDateTime::now_utc(),
            invalidated_at: None,
        });
    }
    out
}

/// Background task driver. Cheap to clone (`Arc`-wrapped).
#[derive(Clone)]
pub struct SummarizationTask {
    cfg: SummarizationConfig,
    events: Arc<dyn EventSource>,
    summaries: Arc<dyn SummaryStore>,
    lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for SummarizationTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SummarizationTask")
            .field("cfg", &self.cfg)
            .field("events", &"<dyn EventSource>")
            .field("summaries", &"<dyn SummaryStore>")
            .field("lock", &"<Mutex<()>>")
            .finish()
    }
}

impl SummarizationTask {
    pub fn new(
        cfg: SummarizationConfig,
        events: Arc<dyn EventSource>,
        summaries: Arc<dyn SummaryStore>,
    ) -> Self {
        Self {
            cfg,
            events,
            summaries,
            lock: Arc::new(Mutex::new(())),
        }
    }

    /// Spawn the periodic loop. Returns immediately. A no-op when
    /// `cfg.enabled == false`.
    pub fn spawn(self) -> Option<tokio::task::JoinHandle<()>> {
        if !self.cfg.enabled {
            info!("summarization disabled by config; task not spawned");
            return None;
        }
        let interval = self.cfg.task_interval;
        Some(tokio::spawn(async move {
            // First run after a short delay so we don't race startup.
            tokio::time::sleep(Duration::from_secs(15)).await;
            loop {
                if let Err(e) = self.run_cycle().await {
                    warn!(error = %e, "summarization cycle failed");
                }
                tokio::time::sleep(interval).await;
            }
        }))
    }

    /// Execute one cycle. Public for tests.
    pub async fn run_cycle(&self) -> Result<u64, String> {
        let Ok(_guard) = self.lock.try_lock() else {
            debug!("previous summarization cycle still running; skipping");
            return Ok(0);
        };
        let start = Instant::now();

        // Optional Ollama probe (kept for D-010); the result only
        // affects the `mechanism` label below.
        let llm_ok = if self.cfg.llm_fallback {
            let client = llm::OllamaClient::new(&self.cfg.ollama_url, &self.cfg.ollama_model);
            match client.probe().await {
                Ok(()) => true,
                Err(e) => {
                    warn!(error = %e, "ollama probe failed; falling back to extractive");
                    false
                }
            }
        } else {
            false
        };

        // Window: last 7 days. The data-model bucket is per-day.
        let cutoff = OffsetDateTime::now_utc() - time::Duration::days(7);
        let records = self
            .events
            .list_recent(cutoff)
            .await
            .map_err(|e| e.to_string())?;
        let summaries = cluster_events(&records);
        let mut written = 0u64;
        for mut s in summaries {
            if s.source_count < self.cfg.event_cluster_min {
                continue;
            }
            if llm_ok {
                s.mechanism = SummaryMechanism::Llm;
            }
            self.summaries
                .upsert_event_summary(&s)
                .await
                .map_err(|e| e.to_string())?;
            written += 1;
            m::incr_summarization_runs(s.mechanism.as_str());
        }
        let lag = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
            - start.elapsed().as_secs_f64();
        m::record_summarization_lag(lag.max(0.0));
        let elapsed_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        info!(written, elapsed_ms, "summarization cycle finished");
        Ok(written)
    }
}

// Re-export a helper convert (used in tests + by ContextBuilder for
// future T039 substitution).

/// Adapter that turns any `Store` into an `EventSource` by listing
/// recent events and projecting `host` + `sub_category` out of the
/// event payload (best-effort: `host`/`machine`, and
/// `sub_category`/`service`/`type`).
#[derive(Clone)]
pub struct StoreEventSource<S: klams_store::Store> {
    store: Arc<S>,
    page_size: u32,
}

impl<S: klams_store::Store> std::fmt::Debug for StoreEventSource<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreEventSource")
            .field("page_size", &self.page_size)
            .finish_non_exhaustive()
    }
}

impl<S: klams_store::Store> StoreEventSource<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            page_size: 500,
        }
    }
}

#[async_trait::async_trait]
impl<S: klams_store::Store> EventSource for StoreEventSource<S> {
    async fn list_recent(
        &self,
        cutoff: OffsetDateTime,
    ) -> Result<Vec<EventRecord>, klams_store::StoreError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let q = klams_store::EventQuery {
                created_after: Some(cutoff),
                limit: self.page_size,
                cursor: cursor.clone(),
                ..Default::default()
            };
            let (events, next) = self.store.list_events(q).await?;
            for e in events {
                let host = payload_str(&e.payload, &["host", "machine"]).unwrap_or("(unknown)");
                let sub =
                    payload_str(&e.payload, &["sub_category", "service", "type"]).unwrap_or("");
                out.push(EventRecord {
                    id: e.id,
                    host: host.to_string(),
                    category: e.category,
                    sub_category: sub.to_string(),
                    at: e.created_at,
                });
            }
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
            if out.len() > 50_000 {
                break;
            }
        }
        Ok(out)
    }
}

fn payload_str<'a>(payload: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    let obj = payload.as_object()?;
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(serde_json::Value::as_str) {
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(host: &str, category: &str, sub: &str, day: i32) -> EventRecord {
        let at =
            OffsetDateTime::from_unix_timestamp(1_700_000_000 + i64::from(day) * 86_400).unwrap();
        EventRecord {
            id: Uuid::now_v7(),
            host: host.into(),
            category: category.into(),
            sub_category: sub.into(),
            at,
        }
    }

    #[test]
    fn cluster_groups_by_host_category_day() {
        let recs = vec![
            rec("kubs0", "pod", "oom", 0),
            rec("kubs0", "pod", "oom", 0),
            rec("kubs0", "pod", "restart", 0),
            rec("kubs0", "pod", "oom", 1),
            rec("kubs1", "pod", "oom", 0),
        ];
        let mut out = cluster_events(&recs);
        out.sort_by_key(|a| (a.host.clone(), a.day_bucket));
        assert_eq!(out.len(), 3);
        let day0 = out
            .iter()
            .find(|s| s.host == "kubs0" && s.source_count == 3)
            .expect("kubs0 day-0 cluster");
        assert!(day0.summary_text.contains("3 events"));
        assert!(day0.summary_text.contains("2× oom"));
    }

    #[test]
    fn cluster_marks_mechanism_as_extractive_by_default() {
        let recs = vec![rec("h", "c", "s", 0)];
        let out = cluster_events(&recs);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].mechanism, SummaryMechanism::Extractive));
    }

    #[test]
    fn cluster_handles_empty_input() {
        let out = cluster_events(&[]);
        assert!(out.is_empty());
    }
}
