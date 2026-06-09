//! Sprint 009 (US1) — contract tests for `[service.limits]`.
//!
//! T1: A client that opens a TCP connection and never sends headers
//!     is closed within `header_read_timeout_secs + 5s`.
//! T2: A client that completes one request and goes silent has its
//!     connection closed within `keep_alive_timeout_secs + 5s`.
//! T3: 100 simultaneous connections from the same peer with
//!     `per_peer_max_concurrent = 8` see >= 92 immediately closed.

use std::time::{Duration, Instant};

use axum::{routing::get, Router};
use klams_service::config::LimitsConfig;
use klams_service::limits::serve_with_limits;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn spawn_server(cfg: LimitsConfig) -> std::net::SocketAddr {
    let router = Router::new().route("/ping", get(|| async { "pong" }));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    // Leak the receiver into a shutdown future so the server lives
    // for the duration of the test; tests drop sender to shut down.
    std::mem::forget(tx);
    tokio::spawn(async move {
        let shutdown = async move {
            let _ = rx.await;
        };
        let _ = serve_with_limits(listener, router, cfg, shutdown).await;
    });
    addr
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_header_read_timeout_closes_silent_client() {
    let cfg = LimitsConfig {
        header_read_timeout_secs: 2,
        keep_alive_timeout_secs: 300,
        per_peer_max_concurrent: 64,
    };
    let addr = spawn_server(cfg).await;

    let start = Instant::now();
    let mut sock = TcpStream::connect(addr).await.expect("connect");

    // Read until EOF without sending anything. The server should
    // close us within ~2s.
    let mut buf = [0u8; 64];
    let result = tokio::time::timeout(Duration::from_secs(7), sock.read(&mut buf)).await;
    let elapsed = start.elapsed();

    let n = result
        .expect("server did not close within 7s")
        .expect("read");
    assert_eq!(n, 0, "expected EOF, got {n} bytes");
    assert!(
        elapsed < Duration::from_secs(7),
        "close took {elapsed:?}, expected < 7s",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t2_keep_alive_timeout_closes_idle_after_request() {
    let cfg = LimitsConfig {
        header_read_timeout_secs: 30,
        keep_alive_timeout_secs: 2,
        per_peer_max_concurrent: 64,
    };
    let addr = spawn_server(cfg).await;

    let mut sock = TcpStream::connect(addr).await.expect("connect");
    sock.write_all(b"GET /ping HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write");
    let mut chunk = vec![0u8; 1024];
    let n = sock.read(&mut chunk).await.expect("read response");
    assert!(n > 0, "expected response bytes");
    assert!(
        std::str::from_utf8(&chunk[..n])
            .unwrap_or("")
            .starts_with("HTTP/1.1 200"),
        "expected 200 response, got: {:?}",
        std::str::from_utf8(&chunk[..n]).unwrap_or(""),
    );

    // Drain anything still buffered, then go silent and wait for EOF.
    let start = Instant::now();
    let mut tail = [0u8; 64];
    let res = tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            match sock.read(&mut tail).await {
                Ok(0) => return Ok::<(), std::io::Error>(()),
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
    })
    .await;
    let elapsed = start.elapsed();
    res.expect("server did not close idle conn within 7s")
        .expect("read err");
    assert!(
        elapsed < Duration::from_secs(7),
        "idle close took {elapsed:?}, expected < 7s",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t3_per_peer_cap_rejects_excess_connections() {
    let cfg = LimitsConfig {
        header_read_timeout_secs: 30,
        keep_alive_timeout_secs: 60,
        per_peer_max_concurrent: 8,
    };
    let addr = spawn_server(cfg).await;

    // Open 100 connections and hold them.
    let mut held: Vec<TcpStream> = Vec::with_capacity(100);
    let mut rejected = 0;
    for _ in 0..100 {
        match TcpStream::connect(addr).await {
            Ok(s) => held.push(s),
            Err(_) => rejected += 1,
        }
    }
    // The OS-level accept always succeeds; the server then closes
    // excess connections immediately. Probe each held socket with a
    // short read — closed peers return EOF quickly.
    let mut closed = 0usize;
    for s in &mut held {
        let mut buf = [0u8; 1];
        let res = tokio::time::timeout(Duration::from_millis(500), s.read(&mut buf)).await;
        if let Ok(Ok(0)) = res {
            closed += 1;
        }
    }
    let total_rejected = rejected + closed;
    assert!(
        total_rejected >= 92,
        "expected >= 92 rejected of 100 (cap=8), got {total_rejected} \
         (os_refused={rejected}, server_closed={closed})",
    );
}
