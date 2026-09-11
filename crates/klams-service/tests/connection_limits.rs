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

/// Sprint 046 (WI #869) — T4: a connection actively STREAMING to the
/// client is not idle, and the keep-alive watchdog must not evict it.
///
/// This is the klams MCP SSE drop reported in #869: a Claude Code
/// client holding the stream open saw it die every ~80s, three times,
/// then the transport close and lazily reconnect. The reported uptimes
/// (77s, 79s x2, 81s, 83s) sit exactly in the window this watchdog
/// produces at its default `keep_alive_timeout_secs = 75` — the tick is
/// `keep_alive / 8`, so eviction lands between 75s and 84.4s.
///
/// The cause is that `IdleTrackedIo` advanced its clock only in
/// `poll_read`. An SSE stream is server -> client only, so no matter how
/// much data the server sends — including rmcp's own 15s SSE keepalive
/// pings, which exist precisely to hold the stream open — the idle
/// clock never moved and the watchdog killed a connection that was
/// busy the whole time.
///
/// The WI proposed testing the loopback origin against ts.net to
/// separate server from proxy. That test was run and pointed here:
/// an idle keep-alive connection to loopback dies at the 30s
/// header-read timeout, not at 80s, so neither observed timer was the
/// tailscale proxy's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_streaming_response_is_not_idle() {
    use axum::response::sse::{Event, Sse};
    use futures::stream;

    let cfg = LimitsConfig {
        header_read_timeout_secs: 30,
        keep_alive_timeout_secs: 2,
        per_peer_max_concurrent: 64,
    };

    // An SSE endpoint that emits a comment every 200ms forever — the
    // shape of rmcp's keepalive-bearing stream, compressed in time.
    let router = Router::new().route(
        "/sse",
        get(|| async {
            Sse::new(stream::unfold((), |()| async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Some((
                    Ok::<_, std::convert::Infallible>(Event::default().comment("ka")),
                    (),
                ))
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = serve_with_limits(listener, router, cfg, std::future::pending::<()>()).await;
    });

    let mut sock = TcpStream::connect(addr).await.expect("connect");
    sock.write_all(b"GET /sse HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write");

    // Read the stream for 5s — well past the 2s keep-alive window —
    // sending NOTHING back, exactly as an SSE client does.
    let start = Instant::now();
    let mut buf = vec![0u8; 4096];
    let mut total = 0usize;
    loop {
        if start.elapsed() >= Duration::from_secs(5) {
            break;
        }
        match tokio::time::timeout(Duration::from_secs(1), sock.read(&mut buf)).await {
            Ok(Ok(0)) => panic!(
                "server closed a STREAMING connection after {:?} — the keep-alive \
                 watchdog counted it idle because the traffic was all server -> client. \
                 This is #869: every held-open MCP SSE stream dies at \
                 keep_alive_timeout_secs (+ up to 1/8 of it in tick slop).",
                start.elapsed()
            ),
            Ok(Ok(n)) => total += n,
            Ok(Err(e)) => panic!("read error after {:?}: {e}", start.elapsed()),
            Err(_) => {} // no data this second; keep waiting
        }
    }
    assert!(
        total > 0,
        "expected streamed bytes from the SSE endpoint, got none"
    );
}

// ---- Sprint 048 (#1806): the mechanism behind `/healthz`'s
// `Connection: close`.
//
// T4 pins the half that is hyper's, not ours: a handler that sets
// `Connection: close` on its response must actually end the connection, not
// merely advertise it. If hyper ever stopped honouring the response header the
// `/healthz` fix would degrade silently back into the pooled keep-alive race —
// the card would flicker again and every unit test would still pass, because
// the header would still be there. This test is what makes that loud.
//
// `keep_alive_timeout_secs` is 300 here deliberately: nothing but the header
// can be what closes the socket within the assertion window.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_connection_close_response_header_ends_the_connection() {
    let cfg = LimitsConfig {
        header_read_timeout_secs: 300,
        keep_alive_timeout_secs: 300,
        per_peer_max_concurrent: 64,
    };
    let router = Router::new().route(
        "/probe",
        get(|| async { ([(axum::http::header::CONNECTION, "close")], "pong") }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    std::mem::forget(tx);
    tokio::spawn(async move {
        let shutdown = async move {
            let _ = rx.await;
        };
        let _ = serve_with_limits(listener, router, cfg, shutdown).await;
    });

    let mut sock = TcpStream::connect(addr).await.expect("connect");
    sock.write_all(b"GET /probe HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write");

    // Read to EOF. A keep-alive connection would sit here until the 300s
    // watchdog; an honoured `Connection: close` ends it as soon as the
    // response is written.
    let start = Instant::now();
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    let res = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match sock.read(&mut buf).await {
                Ok(0) => return Ok::<(), std::io::Error>(()),
                Ok(n) => seen.extend_from_slice(&buf[..n]),
                Err(e) => return Err(e),
            }
        }
    })
    .await;
    let elapsed = start.elapsed();

    res.expect("connection stayed open: `Connection: close` was not honoured (#1806)")
        .expect("read err");
    let text = String::from_utf8_lossy(&seen);
    assert!(
        text.starts_with("HTTP/1.1 200"),
        "expected a 200 before the close, got: {text:?}",
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "close took {elapsed:?}; expected it at response time, not on the watchdog",
    );
}
