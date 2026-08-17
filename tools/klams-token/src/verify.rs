//! `--verify`: is this grant actually live?
//!
//! k-homelab sprint 016 found the `ansible_k` grant returning **401**,
//! while `/etc/ansible/klams.token` and the ansible vault both held a
//! *different* value that was also 401. Something had rotated the grant
//! and neither deployed copy was updated — and there was no way to
//! notice short of trying it. A `list` that printed label,
//! `agent_name` and scopes would have shown that grant looking perfectly healthy,
//! which is why S4 called this the strongest requirement for the tool.
//!
//! The distinction that makes it usable: **403 is healthy.** A grant
//! scoped `["write"]` cannot call a read route, and reporting it dead
//! would train the operator to ignore the column. Only 401 — the token
//! not matching any grant the service holds — means dead.

use klams_types::Scope;
use std::time::Duration;

/// The cheapest authenticated route on the REST surface: it needs
/// `read`, touches no backend, and returns a small body.
const PROBE_PATH: &str = "/memory/policy";

/// What one probe found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// 2xx — the service accepted the token and the scope.
    Live,
    /// 403 — the token authenticated but is not scoped for the probe
    /// route. Healthy; expected for write-only grants.
    LiveScopeLimited,
    /// 401 — the service holds no grant with this token. This is the
    /// dead-`ansible_k` case.
    Dead,
    /// Any other status. Reported verbatim rather than bucketed, since
    /// a 503 during maintenance says nothing about the token.
    Inconclusive { status: u16 },
    /// The probe never got an answer.
    Unreachable { error: String },
}

impl Liveness {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Live => "live".into(),
            Self::LiveScopeLimited => "live (scope-limited)".into(),
            Self::Dead => "DEAD (401)".into(),
            Self::Inconclusive { status } => format!("inconclusive ({status})"),
            Self::Unreachable { .. } => "unreachable".into(),
        }
    }

    /// Whether this result should make the command exit non-zero. Only
    /// a confirmed 401 does: an unreachable service is an operator
    /// problem, not a grant problem, and must not be reported as a dead
    /// credential.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        matches!(self, Self::Dead)
    }
}

/// Classify one HTTP status the way the auth surface means it.
#[must_use]
pub fn classify(status: u16) -> Liveness {
    match status {
        200..=299 => Liveness::Live,
        401 => Liveness::Dead,
        403 => Liveness::LiveScopeLimited,
        other => Liveness::Inconclusive { status: other },
    }
}

/// One authenticated request per grant.
pub async fn probe(client: &reqwest::Client, base_url: &str, token: &str) -> Liveness {
    let url = format!("{}{PROBE_PATH}", base_url.trim_end_matches('/'));
    match client.get(&url).bearer_auth(token).send().await {
        Ok(resp) => classify(resp.status().as_u16()),
        Err(e) => Liveness::Unreachable {
            error: e.to_string(),
        },
    }
}

/// # Errors
/// If the HTTP client cannot be built.
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
}

/// Where to probe, when `--url`/`$KLAMS_URL` were not given: read it
/// out of the very config being inspected. A wildcard bind is not a
/// dialable address, so it becomes loopback.
#[must_use]
pub fn base_url_from_config(listen_addr: &str, port: u16) -> String {
    let host = match listen_addr {
        "0.0.0.0" | "" => "127.0.0.1",
        "::" => "[::1]",
        other if other.contains(':') => return format!("http://[{other}]:{port}"),
        other => other,
    };
    format!("http://{host}:{port}")
}

/// Whether a grant is even *expected* to pass the probe route, used to
/// annotate the report rather than to skip the request — the point of
/// verifying is to ask the service, not to reason about the file.
#[must_use]
pub fn expects_read(scopes: &[Scope]) -> bool {
    scopes.contains(&Scope::Read)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction the whole feature turns on.
    #[test]
    fn only_401_means_dead() {
        assert_eq!(classify(200), Liveness::Live);
        assert_eq!(classify(204), Liveness::Live);
        assert_eq!(classify(403), Liveness::LiveScopeLimited);
        assert_eq!(classify(401), Liveness::Dead);
        assert!(classify(401).is_dead());
        assert!(!classify(403).is_dead());
        assert!(!classify(503).is_dead());
        assert_eq!(classify(503), Liveness::Inconclusive { status: 503 });
    }

    #[test]
    fn an_unreachable_service_is_not_a_dead_grant() {
        let l = Liveness::Unreachable {
            error: "connection refused".into(),
        };
        assert!(!l.is_dead());
    }

    #[test]
    fn wildcard_binds_become_dialable_addresses() {
        assert_eq!(
            base_url_from_config("127.0.0.1", 7777),
            "http://127.0.0.1:7777"
        );
        assert_eq!(
            base_url_from_config("0.0.0.0", 7777),
            "http://127.0.0.1:7777"
        );
        assert_eq!(base_url_from_config("::", 7777), "http://[::1]:7777");
        assert_eq!(base_url_from_config("::1", 7777), "http://[::1]:7777");
    }
}
