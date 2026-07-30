//! Phase 4 — T043: decay config subprocess validation.
//!
//! Spawns the real `klams-service` binary against the live test
//! stack with three different configs and asserts startup
//! behaviour:
//!  (a) invalid `[decay]` (negative λ for TaskFact) → process
//!      exits with code 2, stderr+stdout mentions the offending
//!      key.
//!  (b) valid config → process logs `decay config loaded` and
//!      stays alive; we then SIGTERM it.
//!  (c) tuning λ — two valid configs with different TaskFact λs
//!      both log `decay config loaded` with the configured value,
//!      proving the config flows through to the resolved
//!      `lambda_for(TaskFact)` reported at startup.

#![allow(
    clippy::too_many_lines,
    clippy::float_cmp,
    clippy::manual_assert,
    clippy::needless_pass_by_value,
    clippy::needless_raw_string_hashes,
    clippy::doc_markdown
)]

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind 0");
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

fn binary_path() -> &'static str {
    env!("CARGO_BIN_EXE_klams-service")
}

fn base_config(port: u16, extra_decay: &str) -> String {
    let pg = common::test_pg_url();
    let qd = common::test_qdrant_grpc_url();
    let tei = common::test_tei_url();
    let dim = common::TEST_EMBED_DIM;
    format!(
        r#"
[server]
listen_addr = "127.0.0.1"
port = {port}

[auth]
bearer_token = "test-token-do-not-use-in-prod"

[postgres]
url = "{pg}"
max_connections = 4

[qdrant]
grpc_url = "{qd}"
collection = "knowledge_items_test"

[embeddings]
url = "{tei}"
model_id = "BAAI/bge-small-en-v1.5"
vector_dim = {dim}

[queue]
capacity = 32
workers = 2

[logging]
format = "compact"
level = "info"

[summarization]
enabled = false

{extra_decay}
"#
    )
}

struct Spawned {
    child: Child,
    cfg_path: std::path::PathBuf,
    // Hold tempfile to keep it alive.
    _cfg: tempfile::NamedTempFile,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.cfg_path);
    }
}

fn spawn_service(toml: &str) -> Spawned {
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
    tmp.write_all(toml.as_bytes()).expect("write");
    let path = tmp.path().to_path_buf();
    let mut child = Command::new(binary_path())
        .env("KLAMS_CONFIG", &path)
        .env("RUST_LOG", "info")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn klams-service");

    let stdout_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let so = child.stdout.take().expect("piped stdout");
    let se = child.stderr.take().expect("piped stderr");
    {
        let buf = Arc::clone(&stdout_buf);
        thread::spawn(move || drain(so, buf));
    }
    {
        let buf = Arc::clone(&stderr_buf);
        thread::spawn(move || drain(se, buf));
    }
    Spawned {
        child,
        cfg_path: path,
        _cfg: tmp,
        stdout: stdout_buf,
        stderr: stderr_buf,
    }
}

fn drain<R: Read + Send + 'static>(mut reader: R, buf: Arc<Mutex<String>>) {
    let mut chunk = [0u8; 4096];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if let Ok(mut g) = buf.lock() {
                    g.push_str(&String::from_utf8_lossy(&chunk[..n]));
                }
            }
        }
    }
}

/// Wait up to `timeout` for the process to exit. Returns the
/// captured stdout+stderr and exit status.
fn wait_for_exit(mut s: Spawned, timeout: Duration) -> (String, String, std::process::ExitStatus) {
    let start = Instant::now();
    loop {
        match s.child.try_wait() {
            Ok(Some(status)) => {
                // Give drain threads a beat to flush remaining bytes.
                thread::sleep(Duration::from_millis(100));
                let so = s.stdout.lock().map(|g| g.clone()).unwrap_or_default();
                let se = s.stderr.lock().map(|g| g.clone()).unwrap_or_default();
                let _ = std::fs::remove_file(&s.cfg_path);
                return (so, se, status);
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    panic!("process did not exit within {timeout:?}");
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => panic!("try_wait failed: {e}"),
        }
    }
}

/// Wait up to `timeout` for `needle` to appear in the captured
/// stdout, then for a trailing newline so partial reads can't split
/// the log line we want to parse. Returns the accumulated stdout.
fn wait_for_log(s: &Spawned, needle: &str, timeout: Duration) -> String {
    let start = Instant::now();
    loop {
        let snap = s.stdout.lock().map(|g| g.clone()).unwrap_or_default();
        if let Some(idx) = snap.find(needle) {
            if snap[idx + needle.len()..].contains('\n') {
                return snap;
            }
        }
        if start.elapsed() > timeout {
            let stderr = s.stderr.lock().map(|g| g.clone()).unwrap_or_default();
            panic!(
                "timeout waiting for {needle:?}\n--- stdout ---\n{snap}\n--- stderr ---\n{stderr}"
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "spawns real klams-service binary; requires live test stack"]
fn invalid_decay_lambda_exits_non_zero_with_offending_key() {
    let port = free_port();
    let cfg = base_config(
        port,
        r#"
[decay]
task_interval_seconds = 3600
batch_size = 500
[decay.lambda]
TaskFact = -1.0
"#,
    );
    let s = spawn_service(&cfg);
    let (stdout, stderr, status) = wait_for_exit(s, Duration::from_secs(20));
    eprintln!("exit status: {status:?}");
    eprintln!("--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}");
    assert!(!status.success(), "service should have exited non-zero");
    assert_eq!(status.code(), Some(2), "expected exit code 2");
    let merged = format!("{stdout}{stderr}");
    let lc = merged.to_lowercase();
    assert!(
        lc.contains("decay") && (lc.contains("negative") || lc.contains("lambda")),
        "expected stderr/stdout to mention the offending decay key:\n{merged}"
    );
}

#[test]
#[ignore = "spawns real klams-service binary; requires live test stack"]
fn valid_config_logs_decay_loaded_and_stays_alive() {
    let port = free_port();
    let cfg = base_config(
        port,
        r#"
[decay]
task_interval_seconds = 3600
batch_size = 500
[decay.lambda]
TaskFact = 1.5e-6
"#,
    );
    let mut s = spawn_service(&cfg);
    let log = wait_for_log(&s, "decay config loaded", Duration::from_secs(20));
    eprintln!("--- log captured ---\n{log}");
    assert!(log.contains("task_fact_lambda"));
    let parsed = extract_task_fact_lambda(&log)
        .parse::<f64>()
        .expect("parse lambda");
    assert!(
        (parsed - 1.5e-6).abs() < 1e-10,
        "expected ~1.5e-6, got {parsed}"
    );
    // Process should still be alive.
    assert!(
        s.child.try_wait().expect("try_wait").is_none(),
        "service exited unexpectedly after logging decay config"
    );
}

#[test]
#[ignore = "spawns real klams-service binary; requires live test stack"]
fn tuning_lambda_flows_through_to_startup_log() {
    let port_a = free_port();
    let port_b = free_port();
    let cfg_a = base_config(
        port_a,
        r#"
[decay]
task_interval_seconds = 3600
batch_size = 500
[decay.lambda]
TaskFact = 1.0e-7
"#,
    );
    let cfg_b = base_config(
        port_b,
        r#"
[decay]
task_interval_seconds = 3600
batch_size = 500
[decay.lambda]
TaskFact = 9.0e-5
"#,
    );

    let mut sa = spawn_service(&cfg_a);
    let log_a = wait_for_log(&sa, "decay config loaded", Duration::from_secs(20));
    let _ = sa.child.kill();
    let _ = sa.child.wait();
    let mut sb = spawn_service(&cfg_b);
    let log_b = wait_for_log(&sb, "decay config loaded", Duration::from_secs(20));
    let _ = sb.child.kill();
    let _ = sb.child.wait();

    eprintln!("--- a ---\n{log_a}\n--- b ---\n{log_b}");
    let a = extract_task_fact_lambda(&log_a)
        .parse::<f64>()
        .expect("parse a");
    let b = extract_task_fact_lambda(&log_b)
        .parse::<f64>()
        .expect("parse b");
    assert!(
        (a - 1.0e-7).abs() < 1e-11,
        "log_a should reflect ~1.0e-7, got {a}"
    );
    assert!(
        (b - 9.0e-5).abs() < 1e-9,
        "log_b should reflect ~9.0e-5, got {b}"
    );
    assert_ne!(
        a, b,
        "different configs must produce different resolved lambdas"
    );
}

fn extract_task_fact_lambda(log: &str) -> String {
    // Find the "decay config loaded" line; tracing compact format
    // emits key=value pairs separated by whitespace. We scan the
    // characters after the `task_fact_lambda=` token and collect
    // anything that looks like a float (digits, sign, dot, e/E).
    for line in log.lines() {
        let needle = "task_fact_lambda=";
        if let Some(idx) = line.find(needle) {
            let rest = &line[idx + needle.len()..];
            let val: String = rest
                .chars()
                .take_while(|c| matches!(c, '0'..='9' | '.' | '-' | '+' | 'e' | 'E'))
                .collect();
            if !val.is_empty() {
                return val;
            }
        }
    }
    String::new()
}
