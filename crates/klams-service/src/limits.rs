//! Sprint 009 (US1) — connection limits, per-peer concurrency cap,
//! and a hyper http1 accept loop wired with `header_read_timeout`
//! and an idle keep-alive watchdog.
//!
//! The accept loop replaces the bare `axum::serve` call in
//! [`crate::main`] so we can:
//!
//! 1. Cap concurrent connections per remote IP (drop excess at
//!    accept time — no request bytes are parsed).
//! 2. Apply `Http1Builder::header_read_timeout` so a peer that
//!    opens a TCP connection and never sends headers is reaped
//!    promptly instead of pinning an fd until kernel TCP keep-alive
//!    fires.
//! 3. Apply an idle keep-alive watchdog so a peer that completes a
//!    request and then goes silent is closed within the configured
//!    window.
//!
//! Contract: `specs/009-stability-attribution/contracts/connection-limits.md`.

use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::config::LimitsConfig;

/// Per-peer connection counter. Cheap clone — wraps an `Arc`.
#[derive(Clone, Debug, Default)]
pub struct PerPeerCounter {
    inner: Arc<Mutex<HashMap<IpAddr, u32>>>,
}

impl PerPeerCounter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to reserve a slot for `peer`. Returns `None` if the
    /// peer already holds `>= cap` connections. The returned permit
    /// releases the slot on drop.
    #[must_use]
    pub fn try_acquire(&self, peer: IpAddr, cap: u32) -> Option<PerPeerPermit> {
        let mut guard = self.inner.lock().expect("per-peer mutex poisoned");
        let entry = guard.entry(peer).or_insert(0);
        if *entry >= cap {
            return None;
        }
        *entry += 1;
        Some(PerPeerPermit {
            counter: self.clone(),
            peer,
        })
    }

    /// Active connection count for `peer` (test helper).
    #[must_use]
    pub fn active(&self, peer: IpAddr) -> u32 {
        let guard = self.inner.lock().expect("per-peer mutex poisoned");
        guard.get(&peer).copied().unwrap_or(0)
    }
}

/// RAII permit; drop releases the slot.
#[derive(Debug)]
pub struct PerPeerPermit {
    counter: PerPeerCounter,
    peer: IpAddr,
}

impl Drop for PerPeerPermit {
    fn drop(&mut self) {
        let mut guard = self.counter.inner.lock().expect("per-peer mutex poisoned");
        if let Some(slot) = guard.get_mut(&self.peer) {
            *slot = slot.saturating_sub(1);
            if *slot == 0 {
                guard.remove(&self.peer);
            }
        }
    }
}

/// IO wrapper that records the wall-clock millisecond timestamp of
/// each successful `poll_read`. The accept loop's keep-alive
/// watchdog reads this counter to decide whether to abort an idle
/// connection.
struct IdleTrackedIo<T> {
    inner: T,
    last_read_ms: Arc<AtomicU64>,
}

impl<T> IdleTrackedIo<T> {
    fn new(inner: T, last_read_ms: Arc<AtomicU64>) -> Self {
        last_read_ms.store(now_ms(), Ordering::Relaxed);
        Self {
            inner,
            last_read_ms,
        }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for IdleTrackedIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let pre = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &poll {
            if buf.filled().len() > pre {
                self.last_read_ms.store(now_ms(), Ordering::Relaxed);
            }
        }
        poll
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for IdleTrackedIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Accept-loop entry point. Drives `listener` until `shutdown`
/// resolves, applying the limits in `cfg` to each accepted
/// connection.
///
/// # Errors
/// Returns the first fatal accept error (non-transient I/O failure).
pub async fn serve_with_limits(
    listener: TcpListener,
    router: Router,
    cfg: LimitsConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let header_read = Duration::from_secs(cfg.header_read_timeout_secs);
    let keep_alive = Duration::from_secs(cfg.keep_alive_timeout_secs);
    let per_peer_cap = cfg.per_peer_max_concurrent;
    let counter = PerPeerCounter::new();

    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => {
                info!(target: "klams_service::limits", "shutdown signal received");
                return Ok(());
            }
            res = listener.accept() => {
                let (tcp, peer_addr) = match res {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(target: "klams_service::limits", error = %e, "accept error");
                        continue;
                    }
                };
                let peer_ip = peer_addr.ip();
                let Some(permit) = counter.try_acquire(peer_ip, per_peer_cap) else {
                    warn!(
                        target: "klams_service::limits",
                        event = "connection.per_peer_cap_exceeded",
                        peer = %peer_ip,
                        cap = per_peer_cap,
                        "rejecting connection: per-peer cap exceeded",
                    );
                    drop(tcp);
                    continue;
                };
                let router = router.clone();
                tokio::spawn(serve_one_connection(
                    tcp,
                    peer_addr,
                    router,
                    header_read,
                    keep_alive,
                    permit,
                ));
            }
        }
    }
}

async fn serve_one_connection(
    tcp: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    router: Router,
    header_read: Duration,
    keep_alive: Duration,
    _permit: PerPeerPermit,
) {
    let last_read = Arc::new(AtomicU64::new(now_ms()));
    let io = TokioIo::new(IdleTrackedIo::new(tcp, Arc::clone(&last_read)));
    let svc = TowerToHyperService::new(router);

    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(header_read)
        .keep_alive(true);

    let conn = builder.serve_connection(io, svc).with_upgrades();
    tokio::pin!(conn);

    // Idle watchdog: every `tick` check whether `last_read` has been
    // quiet longer than `keep_alive`. Use a fraction of the window
    // so the eviction lands within the documented `+5s` slop.
    let tick = std::cmp::max(Duration::from_millis(250), keep_alive / 8);

    loop {
        tokio::select! {
            biased;
            res = &mut conn => {
                if let Err(e) = res {
                    // Common: peer reset / EOF. Demote to debug.
                    tracing::debug!(
                        target: "klams_service::limits",
                        peer = %peer,
                        error = %e,
                        "connection ended with error",
                    );
                }
                return;
            }
            () = tokio::time::sleep(tick) => {
                let elapsed = now_ms().saturating_sub(last_read.load(Ordering::Relaxed));
                if u128::from(elapsed) >= keep_alive.as_millis() {
                    info!(
                        target: "klams_service::limits",
                        event = "connection.keep_alive_timeout",
                        peer = %peer,
                        elapsed_ms = elapsed,
                        "closing idle connection",
                    );
                    // Drop the pinned connection by returning; the
                    // owned `conn` and `io` are dropped, closing the
                    // underlying TcpStream.
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_peer_counter_caps_at_limit() {
        let c = PerPeerCounter::new();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let p1 = c.try_acquire(ip, 2).expect("first slot");
        let p2 = c.try_acquire(ip, 2).expect("second slot");
        assert!(c.try_acquire(ip, 2).is_none(), "third must be rejected");
        assert_eq!(c.active(ip), 2);
        drop(p1);
        assert_eq!(c.active(ip), 1);
        let _p3 = c.try_acquire(ip, 2).expect("slot reusable after drop");
        drop(p2);
    }

    #[test]
    fn per_peer_counter_distinguishes_peers() {
        let c = PerPeerCounter::new();
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let _pa = c.try_acquire(a, 1).expect("a");
        let _pb = c.try_acquire(b, 1).expect("b separate bucket");
        assert!(c.try_acquire(a, 1).is_none());
        assert!(c.try_acquire(b, 1).is_none());
    }
}
