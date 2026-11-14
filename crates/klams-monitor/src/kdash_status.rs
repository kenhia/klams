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
//! comes from systemd and so is only known for a scanner whose unit runs
//! on this host — any other scanner reports it as null, which the
//! contract defines as "no source".
//!
//! Inert unless a `[kdash_status]` section is present in the config.

use std::collections::HashMap;
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
    /// The klams author the scanner writes as.
    pub author: String,
    /// The scanner's systemd service, when it runs on THIS host. Its
    /// last successful run becomes `last_ran_at`; absent means null.
    #[serde(default)]
    pub unit: Option<String>,
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
    /// Last successful run per scanner name, from systemd.
    pub last_ran: HashMap<String, i64>,
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
                let wrote = list
                    .iter()
                    .find(|a| a.agent_name == s.author)
                    .map(|a| a.last_seen_at.timestamp())
                    .filter(|t| *t > 0);
                json!({
                    "name": s.name,
                    "last_wrote_at": wrote,
                    "last_ran_at": obs.last_ran.get(&s.name),
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

/// Parse `systemctl show --timestamp=unix -p Result -p ExecMainExitTimestamp`
/// output into the last *successful* run's end time. A unit that has never
/// run reports an empty timestamp; a failed last run is not a run the card
/// should call healthy, so both are `None`.
pub fn parse_last_ran(show: &str) -> Option<i64> {
    let mut result = None;
    let mut exit = None;
    for line in show.lines() {
        if let Some(v) = line.strip_prefix("Result=") {
            result = Some(v.trim());
        } else if let Some(v) = line.strip_prefix("ExecMainExitTimestamp=") {
            exit = v
                .trim()
                .strip_prefix('@')
                .and_then(|n| n.parse::<i64>().ok());
        }
    }
    match (result, exit) {
        (Some("success"), Some(t)) if t > 0 => Some(t),
        _ => None,
    }
}

/// Pull one un-labelled gauge out of Prometheus text exposition.
pub fn parse_gauge(metrics: &str, name: &str) -> Option<f64> {
    metrics.lines().find_map(|l| {
        let rest = l.strip_prefix(name)?;
        let v = rest.strip_prefix(' ')?;
        v.trim().parse::<f64>().ok()
    })
}

/// Reads klams and systemd, builds the record, and publishes it.
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
        for s in &self.cfg.scanners {
            if let Some(unit) = &s.unit {
                if let Some(t) = last_ran(unit).await {
                    obs.last_ran.insert(s.name.clone(), t);
                }
            }
        }
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

async fn last_ran(unit: &str) -> Option<i64> {
    let out = tokio::time::timeout(
        SUBPROCESS_TIMEOUT,
        Command::new("systemctl")
            .args([
                "show",
                unit,
                "--timestamp=unix",
                "-p",
                "Result",
                "-p",
                "ExecMainExitTimestamp",
            ])
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_last_ran(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_ran_success() {
        let s = "Result=success\nExecMainExitTimestamp=@1794618657\n";
        assert_eq!(parse_last_ran(s), Some(1_794_618_657));
    }

    #[test]
    fn last_ran_failed_or_never() {
        assert_eq!(
            parse_last_ran("Result=exit-code\nExecMainExitTimestamp=@1794618657\n"),
            None
        );
        assert_eq!(
            parse_last_ran("Result=success\nExecMainExitTimestamp=\n"),
            None
        );
        assert_eq!(parse_last_ran(""), None);
    }

    #[test]
    fn gauge_parses_only_the_exact_name() {
        let m = "# HELP x\nklams_backup_last_success_timestamp_seconds_total 9\n\
                 klams_backup_last_success_timestamp_seconds 1794556864\n";
        assert_eq!(parse_gauge(m, BACKUP_GAUGE), Some(1_794_556_864.0));
        assert_eq!(parse_gauge("nothing here\n", BACKUP_GAUGE), None);
    }
}
