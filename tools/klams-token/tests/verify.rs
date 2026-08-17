//! `--verify` against a stand-in service.
//!
//! k-homelab sprint 016 found the `ansible_k` grant returning 401 while
//! two deployed copies of "its" token held a different, also-dead
//! value. Nothing could notice, because a `list` that prints label,
//! `agent_name` and scopes shows a dead grant looking perfectly healthy.
//! These tests pin the behaviour that closes that: the tool asks the
//! service, and it does not mistake a scope-limited grant for a dead
//! one.

use std::process::Command;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const BIN: &str = env!("CARGO_BIN_EXE_klams-token");

/// Three grants: one healthy reader, one write-only (403 on the read
/// probe — healthy), one whose token the service no longer holds (401).
const FIXTURE: &str = r#"
[server]
listen_addr = "127.0.0.1"
port = 7777

[[auth.tokens]]
token      = "live-000000000000000000000000"
scopes     = ["read"]
label      = "dashboard"
agent_name = "klams-view"

[[auth.tokens]]
token      = "writeonly-111111111111111111111111"
scopes     = ["write"]
label      = "scanner"
agent_name = "klams-scanner"

[[auth.tokens]]
token      = "dead-222222222222222222222222"
scopes     = ["read", "write"]
label      = "ansible_k"
agent_name = "ansible-k"
"#;

async fn service() -> MockServer {
    let server = MockServer::start().await;
    for (token, status) in [
        ("live-000000000000000000000000", 200),
        ("writeonly-111111111111111111111111", 403),
        ("dead-222222222222222222222222", 401),
    ] {
        Mock::given(method("GET"))
            .and(path("/memory/policy"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
    }
    server
}

#[tokio::test]
async fn verify_reports_live_scope_limited_and_dead_separately() {
    let server = service().await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("klams.toml");
    std::fs::write(&config, FIXTURE).unwrap();

    let out = Command::new(BIN)
        .arg("--config")
        .arg(&config)
        .args(["list", "--json", "--verify", "--url", &server.uri()])
        .output()
        .unwrap();

    let rows: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout should be JSON");
    let rows = rows.as_array().unwrap();

    assert_eq!(rows[0]["liveness"], "live");
    // The distinction the whole feature turns on: a write-only grant
    // cannot call a read route and is NOT dead.
    assert_eq!(rows[1]["liveness"], "live (scope-limited)");
    assert_eq!(rows[2]["liveness"], "DEAD (401)");

    // Exit code 2 — distinct from 1 ("the command failed"), so a
    // monitor can tell a broken credential from a broken config.
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ansible-k"), "{stderr}");
    assert!(stderr.contains("401"), "{stderr}");
}

#[tokio::test]
async fn verify_exits_clean_when_every_grant_answers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/memory/policy"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("klams.toml");
    std::fs::write(&config, FIXTURE).unwrap();

    let out = Command::new(BIN)
        .arg("--config")
        .arg(&config)
        .args(["list", "--verify", "--url", &server.uri()])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("STATUS"), "{stdout}");
    // Still no token values, even in verify output.
    assert!(!stdout.contains("000000000000000000000000"), "{stdout}");
}

/// An unreachable service must not be reported as a fleet of dead
/// credentials — that is an operator problem, not a grant problem.
#[tokio::test]
async fn an_unreachable_service_is_not_reported_as_dead_grants() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("klams.toml");
    std::fs::write(&config, FIXTURE).unwrap();

    let out = Command::new(BIN)
        .arg("--config")
        .arg(&config)
        // Port 1 is reserved and nothing listens there.
        .args(["list", "--json", "--verify", "--url", "http://127.0.0.1:1"])
        .output()
        .unwrap();

    let rows: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    for row in rows.as_array().unwrap() {
        assert_eq!(row["liveness"], "unreachable");
    }
    assert!(out.status.success(), "unreachable is not a failing verdict");
}

/// With no `--url` and no `$KLAMS_URL`, the probe target comes out of
/// the very config being inspected.
#[tokio::test]
async fn the_probe_url_falls_back_to_the_configs_own_server_block() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("klams.toml");
    std::fs::write(&config, FIXTURE).unwrap();

    let out = Command::new(BIN)
        .arg("--config")
        .arg(&config)
        .args(["list", "--json", "--verify"])
        .env_remove("KLAMS_URL")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("http://127.0.0.1:7777"),
        "should have derived the URL from [server]: {stderr}"
    );
}
