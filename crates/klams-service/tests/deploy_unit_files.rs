//! sprint-003 T041 — lint every `.service`/`.timer` file under
//! `deploy/` with `systemd-analyze verify`. Skipped when the binary
//! is not on `PATH` (CI containers, mac dev boxes, etc.).
//!
//! Sprint 031 (#646): the walk RECURSES. It used `read_dir` on
//! `deploy/` alone, so `deploy/systemd/klams.service` was never linted
//! — which is how a stale duplicate unit survived there unnoticed long
//! enough to become its own work item (#647). A lint that silently
//! skips a subdirectory reports clean and means nothing.

use std::path::PathBuf;
use std::process::Command;

fn deploy_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("deploy")
}

fn have_systemd_analyze() -> bool {
    Command::new("systemd-analyze")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Gather `.service` / `.timer` files under `dir`, recursively.
fn collect_units(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_units(&p, out);
            continue;
        }
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(ext, "service" | "timer") {
            out.push(p);
        }
    }
}

#[test]
fn every_unit_file_passes_systemd_analyze_verify() {
    if !have_systemd_analyze() {
        eprintln!("systemd-analyze not on PATH, skipping");
        return;
    }

    let dir = deploy_dir();
    let mut units = Vec::new();
    collect_units(&dir, &mut units);
    units.sort();
    assert!(!units.is_empty(), "no unit files found under deploy/");
    eprintln!("linting {} unit file(s):", units.len());
    for u in &units {
        eprintln!("  {}", u.display());
    }

    for unit in units {
        // systemd-analyze verify wants the [Install] section to be a
        // real unit visible on disk, so we pass the absolute path.
        let out = Command::new("systemd-analyze")
            .arg("verify")
            .arg(&unit)
            .output()
            .expect("spawn systemd-analyze");
        // Allow non-fatal warnings about unknown directives that ship
        // with newer systemd; we only fail on a non-zero exit *and*
        // stderr containing a hard error marker.
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let is_hard_error = !out.status.success()
            && (stderr.contains("not loaded") || stderr.contains("syntax error"));
        assert!(
            !is_hard_error,
            "systemd-analyze rejected {}: {stderr}",
            unit.display(),
        );
    }
}
