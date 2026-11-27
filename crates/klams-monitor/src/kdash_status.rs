//! `kdash:klams:status` publisher (sprint 059, korg WI 4612).
//!
//! Builds the klams status record kdashdata contracts (schema
//! `contracts/schemas/kdash-klams-status.schema.json`, v1, pinned at
//! kdashdata `d57e1dc`) and writes it with `kdash-pub set`, which
//! resolves the central Redis through khlenv, takes the password from
//! `REDISCLI_AUTH` or the host secrets file, and refuses an off-contract
//! key before it opens a socket.
//!
//! It lives in the monitor for the same reason the legacy `[kpidash]`
//! card does: the monitor is an external observer, so when klams does not
//! answer it can still publish `health: "down"` — and the contract asks
//! for exactly that, because silence would read as the publisher being
//! gone rather than klams.
//!
//! Every value is copied from klams, never re-derived: `/healthz` for
//! health, version, uptime, subsystems and queue; `/v1/authors` for the
//! corpus counts (summed exactly as klams-view's `/api/overview` sums
//! them) and each scanner author's `last_seen_at`, which klams bumps only
//! on a write; `/metrics` for the last backup. A scanner's `last_ran_at`
//! is its author's `last_ran_at` on `/v1/authors` — the per-run heartbeat
//! every scanner sends (sprint 061, WI 4639) — so a scanner on any host
//! has one. Before 061 it came from systemd and only this host's scanner
//! had a source. One source for every scanner, on purpose: a run that
//! reported failure, or a scanner that has never sent a heartbeat,
//! publishes null, which the contract defines as "no source".
//!
//! Inert unless a `[kdash_status]` section is present in the config.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use klams_client::Client;
use klams_types::{HealthSnapshot, HealthStatus, ListAuthorsParams, PublicAuthor};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;

/// The contracted key. Version lives in the key; a v2 is a new key.
pub const KEY: &str = "kdash:klams:status";

const INTERVAL_DEFAULT: u64 = 60;
const SCANNER_INTERVAL_DEFAULT: u64 = 3600;
const BACKUP_GAUGE: &str = "klams_backup_last_success_timestamp_seconds";
/// Cursor pages walked when listing authors (200 each).
const AUTHOR_PAGES_MAX: usize = 10;
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(20);

/// `[kdash_status]` section of the monitor config.
#[derive(Debug, Clone, Deserialize)]
pub struct KdashStatusConfig {
    /// Publish cadence in seconds; also written as `expected_refresh_s`.
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// The `kdash-pub` binary. Defaults to `kdash-pub` on `PATH`.
    #[serde(default = "default_kdash_pub")]
    pub kdash_pub: String,
    /// Scanners to report, in the order the card should draw them.
    #[serde(default)]
    pub scanners: Vec<ScannerConfig>,
}

/// One `[[kdash_status.scanners]]` entry.
#[derive(Debug, Clone, Deserialize)]
pub struct ScannerConfig {
    /// Display name on the card — today the scanner's host.
    pub name: String,
    /// The klams author the scanner writes as — and reports its runs
    /// as. (A pre-061 `unit = ` key is ignored: the run time now comes
    /// from the heartbeat for every scanner, not from systemd.)
    pub author: String,
    /// How often the scanner is scheduled, in seconds.
    #[serde(default = "default_scanner_interval")]
    pub expected_interval_s: u64,
}

fn default_interval() -> u64 {
    INTERVAL_DEFAULT
}
fn default_kdash_pub() -> String {
    "kdash-pub".into()
}
fn default_scanner_interval() -> u64 {
    SCANNER_INTERVAL_DEFAULT
}

/// Everything one publish read, gathered before the record is built so
/// the build is a pure function the schema test can drive.
#[derive(Debug, Default)]
pub struct Observed {
    /// Unix seconds the reads started.
    pub ts: f64,
    /// `/healthz`, or `None` when klams did not answer.
    pub health: Option<HealthSnapshot>,
    /// `/v1/authors`, or `None` when the read failed.
    pub authors: Option<Vec<PublicAuthor>>,
    /// The backup gauge from `/metrics`, when present and non-zero.
    pub backup_at: Option<i64>,
}

fn health_word(s: HealthStatus) -> &'static str {
    match s {
        HealthStatus::Ok => "ok",
        HealthStatus::Degraded => "degraded",
        HealthStatus::Down => "down",
    }
}

fn ok(s: HealthStatus) -> bool {
    matches!(s, HealthStatus::Ok)
}

/// Build the contracted record. Every required field is always present;
/// unknowns are present-and-null (kdashdata CD-27).
pub fn build_record(cfg: &KdashStatusConfig, obs: &Observed) -> Value {
    let Some(h) = &obs.health else {
        // klams did not answer: still write, so a fresh `down` says klams
        // is gone and an ageing `ts` says the publisher is.
        return json!({
            "ts": obs.ts,
            "expected_refresh_s": cfg.interval_secs,
            "health": "down",
            "version": null,
            "uptime_s": null,
            "knowledge": null,
            "facts": null,
            "events": null,
            "scanners": [],
            "subsystems": [],
            "queue_depth": null,
            "queue_capacity": null,
            "backup_at": null,
            "new_per_day": [],
        });
    };

    let mut subsystems = vec![
        json!({"name": "postgres", "ok": ok(h.postgres.state)}),
        json!({"name": "qdrant", "ok": ok(h.qdrant.state)}),
        json!({"name": "embed", "ok": ok(h.embeddings.state)}),
    ];
    if let Some(r) = &h.reranker {
        subsystems.push(json!({"name": "rerank", "ok": ok(r.state)}));
    }

    // Counts summed the way klams-view's /api/overview sums them, so the
    // card and the viewer can never disagree about the same corpus.
    let (knowledge, facts, events) = match &obs.authors {
        Some(list) => {
            let (mut k, mut f, mut e) = (0i64, 0i64, 0i64);
            for a in list {
                k += a.counts.knowledge;
                f += a.counts.writes;
                e += a.counts.events;
            }
            (json!(k), json!(f), json!(e))
        }
        None => (Value::Null, Value::Null, Value::Null),
    };

    // Without the author list there is no write time to report; an
    // entry with `last_wrote_at: null` would claim "never wrote".
    let scanners: Vec<Value> = match &obs.authors {
        Some(list) => cfg
            .scanners
            .iter()
            .map(|s| {
                let a = list.iter().find(|a| a.agent_name == s.author);
                let wrote = a.map(|a| a.last_seen_at.timestamp()).filter(|t| *t > 0);
                // Only a run that reported success counts, as with the
                // systemd source this replaced (sprint 059).
                let ran = a
                    .filter(|a| a.last_run_ok == Some(true))
                    .and_then(|a| a.last_ran_at)
                    .map(|t| t.timestamp())
                    .filter(|t| *t > 0);
                json!({
                    "name": s.name,
                    "last_wrote_at": wrote,
                    "last_ran_at": ran,
                    "expected_interval_s": s.expected_interval_s,
                })
            })
            .collect(),
        None => Vec::new(),
    };

    let capacity = (h.queue.capacity > 0).then_some(h.queue.capacity);
    json!({
        "ts": obs.ts,
        "expected_refresh_s": cfg.interval_secs,
        "health": health_word(h.status),
        "version": h.version,
        "uptime_s": h.uptime_seconds,
        "knowledge": knowledge,
        "facts": facts,
        "events": events,
        "scanners": scanners,
        "subsystems": subsystems,
        "queue_depth": capacity.map(|_| h.queue.depth),
        "queue_capacity": capacity,
        "backup_at": obs.backup_at,
        // No klams source for a daily series yet; the contract allows [].
        "new_per_day": [],
    })
}

/// Pull one un-labelled gauge out of Prometheus text exposition.
pub fn parse_gauge(metrics: &str, name: &str) -> Option<f64> {
    metrics.lines().find_map(|l| {
        let rest = l.strip_prefix(name)?;
        let v = rest.strip_prefix(' ')?;
        v.trim().parse::<f64>().ok()
    })
}

/// Reads klams, builds the record, and publishes it.
#[derive(Debug)]
pub struct StatusPublisher {
    cfg: KdashStatusConfig,
    client: Client,
    metrics_url: String,
    http: reqwest::Client,
}

impl StatusPublisher {
    /// `base_url` is the monitor's klams URL; `client` carries its identity.
    pub fn new(cfg: KdashStatusConfig, client: Client, base_url: &str) -> Self {
        Self {
            cfg,
            client,
            metrics_url: format!("{}/metrics", base_url.trim_end_matches('/')),
            http: reqwest::Client::builder()
                .timeout(SUBPROCESS_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.cfg.interval_secs.max(1))
    }

    /// Gather one observation. Never fails: each source that cannot be
    /// read becomes its contract null.
    pub async fn observe(&self) -> Observed {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let health = match self.client.health().await {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(error = %e, "kdash status: klams /healthz unreachable");
                None
            }
        };
        let mut obs = Observed {
            ts,
            health,
            ..Observed::default()
        };
        if obs.health.is_none() {
            return obs;
        }
        obs.authors = match self.authors().await {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!(error = %e, "kdash status: /v1/authors failed");
                None
            }
        };
        obs.backup_at = self.backup_at().await;
        obs
    }

    async fn authors(&self) -> anyhow::Result<Vec<PublicAuthor>> {
        let mut out = Vec::new();
        let mut cursor = None;
        for _ in 0..AUTHOR_PAGES_MAX {
            let page = self
                .client
                .list_authors(&ListAuthorsParams {
                    limit: Some(200),
                    cursor: cursor.take(),
                    ..ListAuthorsParams::default()
                })
                .await?;
            out.extend(page.authors);
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => return Ok(out),
            }
        }
        anyhow::bail!("author list exceeded {AUTHOR_PAGES_MAX} pages")
    }

    async fn backup_at(&self) -> Option<i64> {
        let text = self
            .http
            .get(&self.metrics_url)
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        #[allow(clippy::cast_possible_truncation)]
        parse_gauge(&text, BACKUP_GAUGE)
            .filter(|v| *v >= 1.0)
            .map(|v| v as i64)
    }

    /// Observe, build and publish once. `dry_run` prints instead.
    pub async fn publish_once(&self, dry_run: bool) -> anyhow::Result<Value> {
        let record = build_record(&self.cfg, &self.observe().await);
        let body = serde_json::to_string(&record)?;
        if dry_run {
            println!("{body}");
            return Ok(record);
        }
        let out = tokio::time::timeout(
            SUBPROCESS_TIMEOUT,
            Command::new(&self.cfg.kdash_pub)
                .args(["set", KEY, &body])
                .output(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("{} timed out", self.cfg.kdash_pub))??;
        if !out.status.success() {
            anyhow::bail!(
                "{} set {KEY} exited {}: {}",
                self.cfg.kdash_pub,
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        tracing::debug!(key = KEY, health = %record["health"], "kdash status published");
        Ok(record)
    }

    /// Publish forever on the configured cadence. A failed publish is
    /// logged and retried on the next tick.
    pub async fn run(self) {
        let mut tick = tokio::time::interval(self.interval());
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if let Err(e) = self.publish_once(false).await {
                tracing::warn!(error = %e, "kdash status publish failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_parses_only_the_exact_name() {
        let m = "# HELP x\nklams_backup_last_success_timestamp_seconds_total 9\n\
                 klams_backup_last_success_timestamp_seconds 1794556864\n";
        assert_eq!(parse_gauge(m, BACKUP_GAUGE), Some(1_794_556_864.0));
        assert_eq!(parse_gauge("nothing here\n", BACKUP_GAUGE), None);
    }
}
