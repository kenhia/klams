//! Sprint 042 (#1012) — exercise `deploy/install-from-store.sh` against a
//! fake package store.
//!
//! The script is the one piece of this sprint that runs on a host with no
//! klams checkout, so its failure modes matter more than its happy path:
//! a corrupted download or a mislabelled publish must refuse to install,
//! not install and then lie to `--version` (which is what k-homelab's
//! version floor reads).
//!
//! `curl` speaks `file://`, so the whole store is a temp directory — no
//! network, no server, and the tests run in `just gate`.
//!
//! Skipped when bash/curl/sha256sum are missing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/install-from-store.sh")
}

fn have_tools() -> bool {
    ["bash", "curl", "sha256sum"]
        .iter()
        .all(|c| Command::new(c).arg("--version").output().is_ok())
}

fn suffix() -> String {
    let arch = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout).unwrap();
    let os = String::from_utf8(Command::new("uname").arg("-s").output().unwrap().stdout).unwrap();
    format!("{}-{}", arch.trim(), os.trim().to_lowercase())
}

/// Build `artifacts/<name>/<version>/` holding a stub "binary" that
/// reports `reports_version`, plus a `SHA256SUMS` computed over whatever
/// `corrupt_after_hashing` leaves on disk, plus a `latest` pointer.
fn publish(
    store: &Path,
    name: &str,
    version: &str,
    reports_version: &str,
    corrupt_after_hashing: bool,
) {
    let dir = store.join("artifacts").join(name).join(version);
    std::fs::create_dir_all(&dir).unwrap();
    let file = format!("{name}-{}", suffix());
    let path = dir.join(&file);
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho '{name} {reports_version}'\n"),
    )
    .unwrap();

    let sums = String::from_utf8(
        Command::new("sha256sum")
            .arg(&file)
            .current_dir(&dir)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::fs::write(dir.join("SHA256SUMS"), &sums).unwrap();

    if corrupt_after_hashing {
        std::fs::write(&path, "#!/bin/sh\necho tampered\n").unwrap();
    }

    std::fs::write(store.join("artifacts").join(name).join("latest"), version).unwrap();
}

fn install(store: &Path, dst: &Path, args: &[&str]) -> Output {
    Command::new("bash")
        .arg(script())
        .args(args)
        .env("KLAMS_STORE_URL", format!("file://{}", store.display()))
        .env("BIN_DST_DIR", dst)
        .output()
        .expect("spawn install-from-store.sh")
}

struct Fixture {
    _tmp: tempfile::TempDir,
    store: PathBuf,
    dst: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = tmp.path().join("store");
    let dst = tmp.path().join("bin");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::create_dir_all(&dst).unwrap();
    Fixture {
        _tmp: tmp,
        store,
        dst,
    }
}

#[test]
fn resolves_latest_and_installs() {
    if !have_tools() {
        eprintln!("bash/curl/sha256sum missing, skipping");
        return;
    }
    let f = fixture();
    publish(&f.store, "klams-scanner", "9.9.9", "9.9.9", false);

    let out = install(&f.store, &f.dst, &["klams-scanner"]);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "install failed: {stderr}");

    let installed = f.dst.join("klams-scanner");
    assert!(installed.exists(), "binary was not installed");

    let reported = Command::new(&installed).arg("--version").output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&reported.stdout).trim(),
        "klams-scanner 9.9.9"
    );

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        stdout.contains("resolved latest klams-scanner = 9.9.9"),
        "expected the resolved version to be printed: {stdout}"
    );
    // Installing must not activate: the caller decides when a service
    // takes a new binary, and kai's scanner is timer-driven.
    assert!(
        stdout.contains("Nothing was restarted"),
        "expected the no-restart notice: {stdout}"
    );
}

#[test]
fn rotates_the_outgoing_binary_to_prev() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    publish(&f.store, "klams-scanner", "9.9.8", "9.9.8", false);
    assert!(install(&f.store, &f.dst, &["klams-scanner"])
        .status
        .success());

    publish(&f.store, "klams-scanner", "9.9.9", "9.9.9", false);
    assert!(install(&f.store, &f.dst, &["klams-scanner"])
        .status
        .success());

    let prev = Command::new(f.dst.join("klams-scanner.prev"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&prev.stdout).trim(),
        "klams-scanner 9.9.8",
        ".prev must hold the version that was replaced — it is the fast rollback"
    );
}

#[test]
fn explicit_version_pins_the_fetch() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    publish(&f.store, "klams-scanner", "9.9.8", "9.9.8", false);
    publish(&f.store, "klams-scanner", "9.9.9", "9.9.9", false);

    // `latest` now points at 9.9.9; asking for 9.9.8 is the rollback path.
    let out = install(&f.store, &f.dst, &["--version", "9.9.8", "klams-scanner"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reported = Command::new(f.dst.join("klams-scanner"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&reported.stdout).trim(),
        "klams-scanner 9.9.8"
    );
}

#[test]
fn checksum_mismatch_refuses_to_install() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    publish(&f.store, "klams-scanner", "9.9.9", "9.9.9", true);

    let out = install(&f.store, &f.dst, &["klams-scanner"]);
    assert!(!out.status.success(), "tampered artifact was installed");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("checksum MISMATCH"),
        "expected a checksum error, got: {stderr}"
    );
    assert!(
        !f.dst.join("klams-scanner").exists(),
        "nothing may land in the destination after a failed verification"
    );
}

#[test]
fn mislabelled_version_refuses_to_install() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    // Published as 9.9.9, but the binary itself reports 0.0.1 — a
    // checksum cannot catch this, and it is exactly what would silently
    // defeat k-homelab's version floor.
    publish(&f.store, "klams-scanner", "9.9.9", "0.0.1", false);

    let out = install(&f.store, &f.dst, &["klams-scanner"]);
    assert!(!out.status.success(), "mislabelled artifact was installed");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("reports version 0.0.1"),
        "expected the label mismatch to be named, got: {stderr}"
    );
    assert!(!f.dst.join("klams-scanner").exists());
}

#[test]
fn one_bad_binary_installs_none_of_them() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    publish(&f.store, "klams-service", "9.9.9", "9.9.9", false);
    publish(&f.store, "klams-scanner", "9.9.9", "9.9.9", true); // corrupt

    let out = install(&f.store, &f.dst, &["klams-service", "klams-scanner"]);
    assert!(!out.status.success());
    assert!(
        !f.dst.join("klams-service").exists(),
        "verification must complete for every binary before any is swapped in — \
         a half-applied deploy is worse than a failed one"
    );
}

#[test]
fn missing_store_url_names_the_variable() {
    if !have_tools() {
        return;
    }
    let out = Command::new("bash")
        .arg(script())
        .arg("klams-scanner")
        .env_remove("KLAMS_STORE_URL")
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("KLAMS_STORE_URL"),
        "an unset store URL must name the variable, not fail as a curl error: {stderr}"
    );
}

#[test]
fn no_binary_named_is_an_error() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    let out = install(&f.store, &f.dst, &[]);
    assert!(!out.status.success(), "installed nothing but exited 0");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("at least one binary"),
        "expected a usage error, got: {stderr}"
    );
}

#[test]
fn unpublished_binary_says_so() {
    if !have_tools() {
        return;
    }
    let f = fixture();
    let out = install(&f.store, &f.dst, &["klams-scanner"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("klams-scanner"),
        "the error must name what was missing: {stderr}"
    );
}
