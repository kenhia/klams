//! kpidash health reporter (sprint 010 / US3 T024).
//!
//! Ports the legacy python looper (`ksvc-looper/klams_monitor.py`): poll the
//! klams `/healthz` endpoint and publish a compact status payload to the
//! kpidash Redis dashboard key `kpidash:services:<name>:<host>`. This is the
//! same wire format the `kpidash-client service-status` command writes, so
//! the monitor can take the looper's place without any dashboard changes.
//!
//! Putting this in the monitor (rather than the service) keeps it an external
//! observer: because the Redis sink lives on a separate host, the monitor can
//! still publish `down` for klams-service when the service itself is offline.
//!
//! Compiled by default but entirely inert unless a `[kpidash]` section is
//! present in the monitor config — a fresh clone without a Redis server never
//! attempts a connection and emits no warnings.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use klams_types::{HealthSnapshot, HealthStatus};
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

const ICON_DEFAULT: i64 = 8;
const INTERVAL_DEFAULT: u64 = 30;
const NAME_DEFAULT: &str = "klams";
const HOST_SEGMENT_DEFAULT: &str = "_";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// `[kpidash]` section of the monitor config. Present only when the operator
/// opts in to dashboard reporting.
#[derive(Debug, Clone, Deserialize)]
pub struct KpidashConfig {
    /// Redis host that backs the kpidash dashboard (e.g. `"rpi53"`).
    pub redis_host: String,
    /// Redis port. Defaults to 6379.
    #[serde(default = "default_redis_port")]
    pub redis_port: u16,
    /// Service name — the middle segment of `kpidash:services:<name>:<host>`.
    #[serde(default = "default_name")]
    pub name: String,
    /// Host segment of the Redis key. `"_"` marks a non-host-scoped card.
    #[serde(default = "default_host_segment")]
    pub host: String,
    /// Icon index 0..15 shown on the dashboard card.
    #[serde(default = "default_icon")]
    pub icon: i64,
    /// healthz URL to poll. Defaults to `<monitor url>/healthz`.
    #[serde(default)]
    pub healthz_url: Option<String>,
    /// Report cadence in seconds (the legacy looper used 30).
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    /// Redis password. Falls back to the `REDISCLI_AUTH` env var when unset,
    /// so the secret can stay out of the config file entirely.
    #[serde(default)]
    pub password: Option<String>,
}

fn default_redis_port() -> u16 {
    6379
}
fn default_name() -> String {
    NAME_DEFAULT.into()
}
fn default_host_segment() -> String {
    HOST_SEGMENT_DEFAULT.into()
}
fn default_icon() -> i64 {
    ICON_DEFAULT
}
fn default_interval() -> u64 {
    INTERVAL_DEFAULT
}

/// The compact JSON the dashboard reads. Field order mirrors the python
/// `set_service_status` payload for parity.
#[derive(Debug, Serialize)]
struct Payload {
    ts: f64,
    state: String,
    text: String,
    host: String,
    icon: i64,
}

/// Polls healthz and publishes status to Redis on a fixed cadence.
pub struct Reporter {
    cfg: KpidashConfig,
    healthz_url: String,
    password: Option<String>,
    http: reqwest::Client,
    conn: Option<redis::aio::MultiplexedConnection>,
}

impl std::fmt::Debug for Reporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reporter")
            .field("cfg", &self.cfg)
            .field("healthz_url", &self.healthz_url)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("http", &self.http)
            .field("connected", &self.conn.is_some())
            .finish()
    }
}

impl Reporter {
    /// Build a reporter from config. `monitor_url` is the monitor's klams base
    /// URL, used to derive the healthz endpoint when one isn't set explicitly.
    #[must_use]
    pub fn new(cfg: KpidashConfig, monitor_url: &str) -> Self {
        let healthz_url = cfg
            .healthz_url
            .clone()
            .unwrap_or_else(|| format!("{}/healthz", monitor_url.trim_end_matches('/')));
        let password = cfg
            .password
            .clone()
            .or_else(|| std::env::var("REDISCLI_AUTH").ok());
        if password.is_none() {
            tracing::warn!(
                "kpidash reporter has no Redis password (set [kpidash].password or REDISCLI_AUTH); \
                 will attempt an unauthenticated connection"
            );
        }
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .expect("build reqwest client");
        Self {
            cfg,
            healthz_url,
            password,
            http,
            conn: None,
        }
    }

    /// Configured report cadence.
    #[must_use]
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.cfg.interval_secs)
    }

    /// Poll healthz once and publish the resulting status to Redis.
    pub async fn report_once(&mut self) {
        let (state, text) = self.check_health().await;
        match self.publish(state, &text).await {
            Ok(()) => tracing::debug!(state, text, "kpidash report"),
            Err(e) => tracing::warn!(error = %e, "kpidash publish failed"),
        }
    }

    /// Loop forever, reporting on the configured interval.
    pub async fn run(mut self) {
        let interval = self.interval();
        loop {
            self.report_once().await;
            tokio::time::sleep(interval).await;
        }
    }

    /// Fetch healthz and reduce it to a `(state, text)` pair, matching the
    /// legacy looper's `check_health`.
    async fn check_health(&self) -> (&'static str, String) {
        let resp = match self.http.get(&self.healthz_url).send().await {
            Ok(r) => r,
            Err(e) => return ("down", format!("Unreachable: {e}")),
        };
        // A 503 still carries the snapshot body, so parse regardless of status.
        let snap: HealthSnapshot = match resp.json().await {
            Ok(s) => s,
            Err(e) => return ("down", format!("Unreachable: {e}")),
        };

        if snap.maintenance.as_ref().is_some_and(|m| m.active) {
            return ("maintenance", "Maintenance mode".into());
        }

        let mut problems = Vec::new();
        if snap.status != HealthStatus::Ok {
            problems.push(format!("status={}", status_str(snap.status)));
        }
        for (key, sub) in [
            ("postgres", &snap.postgres),
            ("qdrant", &snap.qdrant),
            ("embeddings", &snap.embeddings),
        ] {
            if sub.state != HealthStatus::Ok {
                problems.push(format!("{key}={}", status_str(sub.state)));
            }
        }
        if !problems.is_empty() {
            return ("unhealthy", problems.join("; "));
        }

        let up = snap.uptime_seconds;
        (
            "ok",
            format!("v{} up {}h{}m", snap.version, up / 3600, (up % 3600) / 60),
        )
    }

    /// Write the status payload to `kpidash:services:<name>:<host>` (no TTL).
    async fn publish(&mut self, state: &str, text: &str) -> Result<()> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());
        let payload = Payload {
            ts,
            state: state.to_string(),
            text: text.to_string(),
            host: self.cfg.host.clone(),
            icon: self.cfg.icon,
        };
        let key = format!("kpidash:services:{}:{}", self.cfg.name, self.cfg.host);
        let body = serde_json::to_string(&payload).context("serialize kpidash payload")?;

        let result = {
            let conn = self.connection().await?;
            conn.set::<_, _, ()>(&key, &body).await
        };
        if let Err(e) = result {
            // Drop the connection so the next report reconnects cleanly.
            self.conn = None;
            return Err(anyhow::Error::new(e).context("redis SET"));
        }
        Ok(())
    }

    /// Lazily (re)establish the multiplexed Redis connection.
    async fn connection(&mut self) -> Result<&mut redis::aio::MultiplexedConnection> {
        if self.conn.is_none() {
            let client = self.build_client()?;
            let conn = client
                .get_multiplexed_async_connection()
                .await
                .context("connect redis")?;
            self.conn = Some(conn);
        }
        Ok(self.conn.as_mut().expect("connection just set"))
    }

    fn build_client(&self) -> Result<redis::Client> {
        let info = redis::ConnectionInfo {
            addr: redis::ConnectionAddr::Tcp(self.cfg.redis_host.clone(), self.cfg.redis_port),
            redis: redis::RedisConnectionInfo {
                db: 0,
                username: None,
                password: self.password.clone(),
                ..Default::default()
            },
        };
        redis::Client::open(info).context("open redis client")
    }
}

fn status_str(s: HealthStatus) -> &'static str {
    match s {
        HealthStatus::Ok => "Ok",
        HealthStatus::Degraded => "Degraded",
        HealthStatus::Down => "Down",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> KpidashConfig {
        KpidashConfig {
            redis_host: "rpi53".into(),
            redis_port: 6379,
            name: "klams".into(),
            host: "_".into(),
            icon: 8,
            healthz_url: None,
            interval_secs: 30,
            password: None,
        }
    }

    #[test]
    fn healthz_url_derived_from_monitor_url() {
        let r = Reporter::new(cfg(), "http://127.0.0.1:7777/");
        assert_eq!(r.healthz_url, "http://127.0.0.1:7777/healthz");
    }

    #[test]
    fn explicit_healthz_url_wins() {
        let mut c = cfg();
        c.healthz_url = Some("http://example/h".into());
        let r = Reporter::new(c, "http://127.0.0.1:7777");
        assert_eq!(r.healthz_url, "http://example/h");
    }

    #[test]
    fn status_str_matches_wire_names() {
        assert_eq!(status_str(HealthStatus::Ok), "Ok");
        assert_eq!(status_str(HealthStatus::Degraded), "Degraded");
        assert_eq!(status_str(HealthStatus::Down), "Down");
    }

    #[test]
    fn payload_serializes_in_dashboard_order() {
        let p = Payload {
            ts: 1.5,
            state: "ok".into(),
            text: "v0.1.0 up 1h2m".into(),
            host: "_".into(),
            icon: 8,
        };
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(
            s,
            r#"{"ts":1.5,"state":"ok","text":"v0.1.0 up 1h2m","host":"_","icon":8}"#
        );
    }

    #[test]
    fn config_defaults_apply() {
        let c: KpidashConfig = toml::from_str(r#"redis_host = "rpi53""#).unwrap();
        assert_eq!(c.redis_port, 6379);
        assert_eq!(c.name, "klams");
        assert_eq!(c.host, "_");
        assert_eq!(c.icon, 8);
        assert_eq!(c.interval_secs, 30);
        assert!(c.healthz_url.is_none());
        assert!(c.password.is_none());
    }
}
