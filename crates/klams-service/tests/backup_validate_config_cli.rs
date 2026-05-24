//! Sprint 006 T013 — `klams-service --validate-backup-config`
//! end-to-end CLI check.
//!
//! Builds the binary via `cargo run` and invokes it with a temp
//! `klams.toml`. Validates the documented contract: exit code `0`
//! and `OK:` line on a valid config; exit code `2` on an invalid
//! `[backup]` block.

#![cfg(test)]

use std::io::Write;
use std::process::Command;
use tempfile::NamedTempFile;

fn write_cfg(body: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("tempfile");
    write!(f, "{body}").expect("write");
    f.flush().expect("flush");
    f
}

fn minimal_cfg(extra: &str) -> String {
    // Mirrors deploy/config/klams.example.toml — only the blocks
    // required by Config deserialization plus whatever `extra` adds.
    format!(
        r#"
[server]
listen_addr = "127.0.0.1"
port = 7777

[auth]
bearer_token = "test-token"

[postgres]
url = "postgres://klams:klams@127.0.0.1:5432/klams"
max_connections = 4

[qdrant]
grpc_url = "http://127.0.0.1:6334"
collection = "knowledge_items"

[embeddings]
url = "http://127.0.0.1:7070"
model_id = "BAAI/bge-small-en-v1.5"
vector_dim = 384

[queue]
capacity = 256
workers = 2

[logging]
format = "json"
level = "info"

[decay.lambda]
UserFact = 1e-9
TaskFact = 1e-6
EnvFact  = 1e-9

{extra}
"#
    )
}

fn run_validate(cfg_path: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO"))
        .args([
            "run",
            "--quiet",
            "-p",
            "klams-service",
            "--",
            "--validate-backup-config",
        ])
        .env("KLAMS_CONFIG", cfg_path)
        .output()
        .expect("spawn cargo run")
}

#[test]
#[ignore = "rebuilds klams-service binary; slow"]
fn validate_backup_config_disabled_succeeds() {
    let cfg = write_cfg(&minimal_cfg(""));
    let out = run_validate(cfg.path());
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("OK:"),
        "expected OK in stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("disabled"),
        "expected 'disabled' in stdout, got: {stdout}"
    );
}

#[test]
#[ignore = "rebuilds klams-service binary; slow"]
fn validate_backup_config_enabled_writable_dir_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let extra = format!(
        r#"
[backup]
enabled = true
backup_dir = "{}"
window_start_utc = "07:00"
"#,
        dir.path().display()
    );
    let cfg = write_cfg(&minimal_cfg(&extra));
    let out = run_validate(cfg.path());
    assert!(
        out.status.success(),
        "expected exit 0, got {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK:"), "stdout: {stdout}");
    assert!(stdout.contains("enabled"), "stdout: {stdout}");
}

#[test]
#[ignore = "rebuilds klams-service binary; slow"]
fn validate_backup_config_enabled_no_dir_exits_2() {
    let extra = r#"
[backup]
enabled = true
window_start_utc = "07:00"
"#;
    let cfg = write_cfg(&minimal_cfg(extra));
    let out = run_validate(cfg.path());
    assert_eq!(out.status.code(), Some(2), "expected exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("backup_dir is unset"), "stderr: {stderr}");
}

#[test]
#[ignore = "rebuilds klams-service binary; slow"]
fn validate_backup_config_invalid_window_start_exits_2() {
    let extra = r#"
[backup]
enabled = false
window_start_utc = "25:00"
"#;
    let cfg = write_cfg(&minimal_cfg(extra));
    let out = run_validate(cfg.path());
    assert_eq!(out.status.code(), Some(2), "expected exit 2");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("hour must be 0..=23"), "stderr: {stderr}");
}
