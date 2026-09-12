//! Tailnet cross-check: which node did this request come from?
//! (Sprint 049, korg WI 2389.)
//!
//! klams authenticates by *declared name* (sprint 049 — see
//! [`crate::auth`]), and a name says nothing about where it was
//! declared from. `tailscale whois` closes that gap: the caller's
//! tailnet address resolves to a node, which is recorded beside the
//! declared `agent_name` so "why did a write from `claude` show up
//! here" is answerable after the fact.
//!
//! **It is data, not a check.** A whois failure never refuses a
//! request — tailscaled being down, the binary being absent, the
//! address being unknown to the tailnet, and whois being switched off
//! all resolve to the same thing: `unknown`. The optional enforcement
//! path in [`crate::auth`] is off by default and is the only place the
//! answer can ever affect an outcome.
//!
//! ## Where the address comes from
//!
//! Measured on kubs0, 2026-09-12: klams runs behind `tailscale serve`
//! (`https://kubs0…:7777` → `proxy http://localhost:7777`), so the
//! socket peer is always `127.0.0.1` and carries no information. What
//! `serve` forwards is `X-Forwarded-For: <tailnet ip>` — verified by
//! standing a throwaway `serve` listener up, curling it **from kai**,
//! and reading the headers that arrived. So `X-Forwarded-For` is the
//! primary source and the socket peer is the fallback for a direct
//! (non-`serve`) deployment.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Resolves a tailnet address to a short node name (`kai`, `kubs0`).
///
/// A trait so the auth middleware can be tested without a tailnet, and
/// so a deployment with no `tailscale` binary can install nothing at
/// all rather than pay a failed process spawn per address.
#[async_trait::async_trait]
pub trait NodeResolver: Send + Sync + std::fmt::Debug {
    /// The node `addr` belongs to, or `None` if it cannot be resolved.
    async fn resolve(&self, addr: IpAddr) -> Option<String>;
}

/// Extract the short node name from `tailscale whois --json` output.
///
/// The short name is the first DNS label of `Node.Name`, which arrives
/// fully qualified and trailing-dotted (`kai.encke-wahoo.ts.net.`).
///
/// Returns `None` for anything that is not that — notably the literal
/// `peer not found` line whois prints for an address the tailnet does
/// not know, which is **not** JSON. "Did not parse" and "command
/// failed" are deliberately the same outcome: both mean the node is
/// unknown, and distinguishing them would only tempt a caller into
/// treating one of them as an error worth failing on.
#[must_use]
pub fn parse_node_name(stdout: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    let name = parsed.get("Node")?.get("Name")?.as_str()?;
    let short = name.split('.').next()?.trim();
    if short.is_empty() {
        return None;
    }
    Some(short.to_string())
}

/// The outcome of a cache lookup.
///
/// Three states, not two: absent-or-expired is a `Miss`, but a live
/// entry can itself say "this address does not resolve" — and that
/// negative answer is the one worth caching, because otherwise an
/// unresolvable peer spawns a process on every request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CacheLookup {
    Miss,
    Hit(Option<String>),
}

/// [`NodeResolver`] backed by the `tailscale` CLI, with a TTL cache.
///
/// The cache holds negative answers too: an address the tailnet cannot
/// resolve would otherwise spawn a process on **every** request from
/// it, which is exactly the caller you least want to pay for.
#[derive(Debug)]
pub struct TailscaleWhois {
    binary: String,
    ttl: Duration,
    cache: Mutex<HashMap<IpAddr, (Instant, Option<String>)>>,
}

impl TailscaleWhois {
    /// Resolver invoking `tailscale` from `PATH`, caching for `ttl`.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self::with_binary("tailscale", ttl)
    }

    /// As [`Self::new`], with an explicit binary path.
    #[must_use]
    pub fn with_binary(binary: impl Into<String>, ttl: Duration) -> Self {
        Self {
            binary: binary.into(),
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn cached(&self, addr: IpAddr) -> CacheLookup {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((at, value)) = cache.get(&addr) else {
            return CacheLookup::Miss;
        };
        if at.elapsed() > self.ttl {
            return CacheLookup::Miss;
        }
        CacheLookup::Hit(value.clone())
    }

    fn store(&self, addr: IpAddr, value: Option<String>) {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(addr, (Instant::now(), value));
    }
}

#[async_trait::async_trait]
impl NodeResolver for TailscaleWhois {
    async fn resolve(&self, addr: IpAddr) -> Option<String> {
        if let CacheLookup::Hit(node) = self.cached(addr) {
            return node;
        }
        let output = tokio::process::Command::new(&self.binary)
            .arg("whois")
            .arg("--json")
            .arg(addr.to_string())
            .output()
            .await;
        let node = match output {
            Ok(out) => parse_node_name(&String::from_utf8_lossy(&out.stdout)),
            Err(e) => {
                // Debug, not warn: on a host with no tailscale this
                // fires once per address per TTL forever, and it is a
                // supported configuration (`[auth.whois] enabled =
                // false` is the way to silence it deliberately).
                tracing::debug!(
                    error = %e,
                    binary = %self.binary,
                    "tailscale whois could not be invoked; recording node as unknown"
                );
                None
            }
        };
        self.store(addr, node.clone());
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixtures measured against the live tailnet on 2026-09-12, cut
    /// down to the fields this parser reads.
    const TAGGED_NODE: &str = r#"{
      "Node": {
        "ID": 8503781158862512,
        "Name": "kai.encke-wahoo.ts.net.",
        "Tags": ["tag:server"],
        "Hostinfo": { "Hostname": "kai" }
      },
      "UserProfile": { "LoginName": "tagged-devices" }
    }"#;

    const USER_DEVICE: &str = r#"{
      "Node": { "Name": "cleo.encke-wahoo.ts.net.", "Tags": null },
      "UserProfile": { "LoginName": "ken.hiatt@gmail.com" }
    }"#;

    #[test]
    fn parses_short_name_from_a_tagged_node() {
        assert_eq!(parse_node_name(TAGGED_NODE).as_deref(), Some("kai"));
    }

    /// A user-owned device has no `Tags` and a real `LoginName`; the
    /// node name is read the same way regardless, because the node —
    /// not the human — is the fact being recorded.
    #[test]
    fn parses_short_name_from_a_user_device() {
        assert_eq!(parse_node_name(USER_DEVICE).as_deref(), Some("cleo"));
    }

    /// The measured failure shape: whois prints `peer not found` on
    /// stdout, as a bare line, not as JSON.
    #[test]
    fn unknown_peer_output_is_not_an_error_it_is_unknown() {
        assert_eq!(parse_node_name("peer not found\n"), None);
    }

    #[test]
    fn malformed_or_empty_output_is_unknown() {
        for junk in ["", "   ", "{}", r#"{"Node":{}}"#, r#"{"Node":{"Name":""}}"#] {
            assert_eq!(parse_node_name(junk), None, "input {junk:?}");
        }
    }

    /// A resolver that counts calls, so the cache can be observed.
    #[derive(Debug, Default)]
    struct CountingResolver {
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl NodeResolver for CountingResolver {
        async fn resolve(&self, _addr: IpAddr) -> Option<String> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some("kai".into())
        }
    }

    #[tokio::test]
    async fn trait_object_is_usable_from_the_middleware() {
        let r: std::sync::Arc<dyn NodeResolver> = std::sync::Arc::new(CountingResolver::default());
        assert_eq!(
            r.resolve("100.97.109.60".parse().unwrap()).await.as_deref(),
            Some("kai")
        );
    }

    /// A miss is cached as a miss. Without this, an address the tailnet
    /// cannot resolve spawns a process on every single request from it.
    #[tokio::test]
    async fn negative_answers_are_cached_too() {
        // `true` exits 0 with empty stdout — an unparseable answer, so
        // the resolved value is `None`.
        let w = TailscaleWhois::with_binary("true", Duration::from_secs(60));
        let addr: IpAddr = "100.64.0.99".parse().unwrap();
        assert_eq!(w.resolve(addr).await, None);
        assert_eq!(
            w.cached(addr),
            CacheLookup::Hit(None),
            "a `None` answer must still occupy a cache slot, or every \
             request from an unresolvable peer spawns a process"
        );
    }

    /// A binary that does not exist resolves to `unknown` rather than
    /// erroring — the "no tailscale on this host" deployment.
    #[tokio::test]
    async fn a_missing_binary_resolves_to_unknown() {
        let w = TailscaleWhois::with_binary(
            "definitely-not-a-real-binary-klams-049",
            Duration::from_secs(60),
        );
        assert_eq!(w.resolve("100.97.109.60".parse().unwrap()).await, None);
    }

    #[tokio::test]
    async fn expired_entries_are_not_served() {
        let w = TailscaleWhois::with_binary("true", Duration::from_millis(1));
        let addr: IpAddr = "100.64.0.99".parse().unwrap();
        w.store(addr, Some("stale".into()));
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(
            w.cached(addr),
            CacheLookup::Miss,
            "an expired entry must be a miss"
        );
    }
}
