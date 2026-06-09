//! Loopback half-close soak harness (sprint 009, FR-004 / SC-001).
//!
//! Opens N concurrent TCP connections to the target service, sends
//! partial HTTP request headers, then closes the client write half
//! without reading. Repeats at the configured rate for the
//! configured duration. Periodically samples fd count and
//! `CLOSE_WAIT` count for the target process via `pidof`/`/proc`/`ss`
//! and prints them as JSON lines for downstream analysis.
//!
//! Used for:
//!   * `just soak --duration 10m` — fast feedback during dev.
//!   * `just soak --duration 18h` — overnight SC-001 verdict.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::process::Command;

/// Loopback half-close soak harness.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Target `host:port` for klams-service.
    #[arg(long, default_value = "127.0.0.1:7777")]
    target: String,

    /// Total soak duration (e.g. `10m`, `18h`, `30s`).
    #[arg(long, default_value = "10m")]
    duration: humantime::Duration,

    /// Maximum number of in-flight half-open connections.
    #[arg(long, default_value_t = 32)]
    concurrency: u32,

    /// New-connection arrival rate, in connections per second.
    #[arg(long, default_value_t = 4)]
    rate: u32,

    /// Sample `fd/CLOSE_WAIT` every N seconds.
    #[arg(long, default_value_t = 30)]
    sample_interval_secs: u64,

    /// Service binary name used to locate the pid for /proc sampling.
    #[arg(long, default_value = "klams-service")]
    process_name: String,
}

#[derive(Debug, Default)]
struct Counters {
    opened: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let total: Duration = args.duration.into();
    let interval = if args.rate == 0 {
        Duration::from_millis(250)
    } else {
        Duration::from_secs_f64(1.0 / f64::from(args.rate))
    };
    let semaphore = Arc::new(tokio::sync::Semaphore::new(args.concurrency as usize));
    let counters = Arc::new(Counters::default());
    let start = Instant::now();
    let deadline = start + total;

    eprintln!(
        "klams-soak: target={} duration={:?} concurrency={} rate={}/s",
        args.target, total, args.concurrency, args.rate,
    );

    let sampler_counters = Arc::clone(&counters);
    let sampler_process = args.process_name.clone();
    let sampler_target = args.target.clone();
    let sampler_interval = args.sample_interval_secs.max(1);
    let sampler = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(sampler_interval));
        loop {
            tick.tick().await;
            sample(&sampler_process, &sampler_target, &sampler_counters, start).await;
            if Instant::now() >= deadline {
                break;
            }
        }
    });

    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while Instant::now() < deadline {
        tick.tick().await;
        let Ok(permit) = Arc::clone(&semaphore).acquire_owned().await else {
            break;
        };
        let target = args.target.clone();
        let counters = Arc::clone(&counters);
        counters.opened.fetch_add(1, Ordering::Relaxed);
        tokio::spawn(async move {
            let _permit = permit;
            match half_close_once(&target).await {
                Ok(()) => {
                    counters.completed.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    counters.failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }

    sampler.abort();
    sample(&args.process_name, &args.target, &counters, start).await;
    println!(
        "{{\"event\":\"soak.done\",\"opened\":{},\"completed\":{},\"failed\":{},\"elapsed_secs\":{}}}",
        counters.opened.load(Ordering::Relaxed),
        counters.completed.load(Ordering::Relaxed),
        counters.failed.load(Ordering::Relaxed),
        start.elapsed().as_secs(),
    );
    Ok(())
}

async fn half_close_once(target: &str) -> Result<()> {
    let mut sock = TcpStream::connect(target)
        .await
        .with_context(|| format!("connect {target}"))?;
    let _ = sock
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: soak\r\n")
        .await;
    let _ = sock.shutdown().await;
    Ok(())
}

async fn sample(process: &str, target: &str, counters: &Counters, start: Instant) {
    let pid = pid_of(process).await;
    let fd_count = pid.and_then(fd_count_for_pid).unwrap_or(0);
    let close_wait = close_wait_for(target).await.unwrap_or(0);
    println!(
        "{{\"event\":\"soak.sample\",\"elapsed_secs\":{},\"pid\":{},\"fd_count\":{},\"close_wait\":{},\"opened\":{},\"completed\":{},\"failed\":{}}}",
        start.elapsed().as_secs(),
        pid.unwrap_or(0),
        fd_count,
        close_wait,
        counters.opened.load(Ordering::Relaxed),
        counters.completed.load(Ordering::Relaxed),
        counters.failed.load(Ordering::Relaxed),
    );
}

async fn pid_of(process: &str) -> Option<u32> {
    let out = Command::new("pidof").arg(process).output().await.ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    s.split_whitespace().next()?.parse::<u32>().ok()
}

fn fd_count_for_pid(pid: u32) -> Option<u64> {
    let entries = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
    Some(u64::try_from(entries.filter_map(Result::ok).count()).unwrap_or(u64::MAX))
}

async fn close_wait_for(target: &str) -> Option<u64> {
    let out = Command::new("ss")
        .args(["-tan", "state", "close-wait", "dst", target])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let lines = s.lines().count();
    Some(u64::try_from(lines.saturating_sub(1)).unwrap_or(0))
}
