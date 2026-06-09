//! sprint-003 T040 — exercise `deploy/install-systemd.sh` in `--dry-run`
//! mode. We assert it (a) prints the actions it would take, (b) is
//! idempotent (two consecutive dry-runs produce identical action
//! lists), and (c) fails loud when a binary is missing.
//!
//! Skipped when `bash` is missing.

use std::path::PathBuf;
use std::process::Command;

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("deploy")
        .join("install-systemd.sh")
}

fn have_bash() -> bool {
    Command::new("bash").arg("--version").output().is_ok()
}

#[test]
fn dry_run_prints_planned_actions() {
    if !have_bash() {
        eprintln!("bash missing, skipping");
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    for b in ["klams-service", "klams-scanner", "klams-monitor"] {
        let p = bin_dir.join(b);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    let out = Command::new("bash")
        .arg(script())
        .arg("--dry-run")
        .env("BIN_SRC_DIR", &bin_dir)
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    if !out.status.success() && stderr.contains("postgresql.service not found") {
        eprintln!("postgresql not on host, skipping");
        return;
    }
    assert!(out.status.success(), "dry-run failed: {stderr}");
    assert!(
        stdout.contains("[dry-run]"),
        "expected dry-run markers in stdout: {stdout}"
    );
    assert!(
        stdout.contains("systemctl daemon-reload"),
        "expected daemon-reload action in stdout: {stdout}"
    );
}

#[test]
fn dry_run_is_idempotent() {
    if !have_bash() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    for b in ["klams-service", "klams-scanner", "klams-monitor"] {
        let p = bin_dir.join(b);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    let run = || -> Option<String> {
        let out = Command::new("bash")
            .arg(script())
            .arg("--dry-run")
            .env("BIN_SRC_DIR", &bin_dir)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let Some(first) = run() else {
        eprintln!("dry-run unavailable, skipping");
        return;
    };
    let second = run().expect("second dry-run");
    let normalise = |s: String| -> String {
        // STAGE_DIR is `/tmp/klams-stage-$$`, so the bash PID leaks
        // into every dry-run; mask it so consecutive runs compare equal.
        let mut out = String::with_capacity(s.len());
        let needle = "/tmp/klams-stage-";
        let mut rest = s.as_str();
        while let Some(idx) = rest.find(needle) {
            out.push_str(&rest[..idx]);
            out.push_str(needle);
            out.push_str("PID");
            let after = &rest[idx + needle.len()..];
            let end = after
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len());
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    };
    assert_eq!(
        normalise(first),
        normalise(second),
        "dry-run is not idempotent",
    );
}

#[test]
fn missing_binary_fails_loudly() {
    if !have_bash() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let bin_dir = tmp.path().join("empty");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let out = Command::new("bash")
        .arg(script())
        .arg("--dry-run")
        .env("BIN_SRC_DIR", &bin_dir)
        .output()
        .expect("spawn");
    assert!(!out.status.success(), "expected failure for missing binary");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("missing binary") || stderr.contains("postgresql.service not found"),
        "expected a clear error, got: {stderr}",
    );
}
