//! Sprint 042 (#1012) — `klams-service --version` must answer without a
//! config file.
//!
//! `--version` is the homelab's freshness signal: k-homelab's
//! `recipes/klams-scanner` reads it to enforce a version floor, and
//! `deploy/install-from-store.sh` reads it to prove a fetched binary
//! carries the label it was published under. A binary that needs a
//! config (and a bearer token) before it will say what it is cannot
//! participate in either check — and on a host being provisioned, the
//! config does not exist yet.
//!
//! `klams-scanner` and `klams-monitor` get this for free from clap.
//! `klams-service` parses its flags by hand, so it needs the early-out
//! these tests pin down.

use std::process::Command;

/// Point config resolution somewhere that certainly does not exist, so
/// the test is independent of whatever config the host happens to have.
/// `resolve_config_path` returns `$KLAMS_CONFIG` verbatim, so this
/// reaches (and fails) the config *load* — which is exactly the
/// failure `--version` must come before.
fn run_with_no_config(flag: &str) -> std::process::Output {
    let missing = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("no-such-dir-sprint-042")
        .join("klams.toml");
    Command::new(env!("CARGO_BIN_EXE_klams-service"))
        .arg(flag)
        .env("KLAMS_CONFIG", &missing)
        .output()
        .expect("spawn klams-service")
}

#[test]
fn long_version_flag_prints_name_and_version_without_config() {
    let out = run_with_no_config("--version");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    assert!(
        out.status.success(),
        "--version exited non-zero without a config: {stderr}"
    );
    assert_eq!(
        stdout.trim(),
        format!("klams-service {}", env!("CARGO_PKG_VERSION")),
        "expected clap's `<name> <version>` shape, so the fleet's \
         `awk '{{print $NF}}'` readers keep working"
    );
}

#[test]
fn short_version_flag_matches_long() {
    let long = run_with_no_config("--version");
    let short = run_with_no_config("-V");
    assert!(short.status.success(), "-V exited non-zero");
    assert_eq!(
        String::from_utf8_lossy(&short.stdout),
        String::from_utf8_lossy(&long.stdout),
        "-V is clap's short form for --version; the hand-rolled parser \
         must accept both"
    );
}

#[test]
fn version_flag_beats_other_flags_on_the_same_line() {
    // The early-out must come before every other flag's handling, or a
    // provisioning host running `--version` alongside anything else
    // still trips config loading. Ordering, not parsing, is the thing
    // under test.
    let missing = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("no-such-dir/x.toml");
    let out = Command::new(env!("CARGO_BIN_EXE_klams-service"))
        .args(["--validate-config", "--version"])
        .env("KLAMS_CONFIG", &missing)
        .output()
        .expect("spawn");
    assert!(
        out.status.success(),
        "--version did not short-circuit --validate-config: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("klams-service "));
}
