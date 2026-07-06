//! sprint-003 T048 — handoff directory layout contract tests.
//!
//! All four checks from `contracts/handoff_index.md § Acceptance`.
//! Three are pure-fs (no network); the fourth posts a real `UserFact`
//! when `KLAMS_URL` is set, and skips otherwise.

use std::path::PathBuf;
use std::process::Command;

fn handoff_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("sprints")
        .join("003-non-agentic-writes")
        .join("handoff")
}

#[test]
fn handoff_directory_layout_matches_contract() {
    let d = handoff_dir();
    for required in [
        "README.md",
        "spec.md",
        "api-contract.md",
        "examples/post-userfact.sh",
    ] {
        let p = d.join(required);
        assert!(
            p.is_file(),
            "missing required handoff file: {}",
            p.display()
        );
    }
}

#[test]
fn handoff_pinned_version_header_present() {
    let body = std::fs::read_to_string(handoff_dir().join("README.md")).unwrap();
    let expected = "This document is pinned to klams sprint-003 API surface\n\
                    (sprints/003-non-agentic-writes/spec.md in the klams repo).\n\
                    If GET /healthz?contract=v1 ever returns anything other than\n\
                    200 with {\"contract\":\"v1\"}, the contract this document describes\n\
                    is no longer guaranteed.";
    assert!(
        body.contains(expected),
        "README.md is missing the pinned-version header verbatim",
    );
}

#[test]
fn handoff_api_contract_lists_required_failure_modes() {
    let body = std::fs::read_to_string(handoff_dir().join("api-contract.md")).unwrap();
    for marker in [
        "| `200`",
        "| `202`",
        "| `409`",
        "| `422`",
        "| `5xx`",
        "canonical write",
        "diverted to dissents",
        "optimistic-concurrency mismatch",
        "validation error",
        "retry with",
    ] {
        assert!(
            body.contains(marker),
            "api-contract.md missing failure-mode marker: {marker}",
        );
    }
}

#[test]
fn handoff_example_script_posts_userfact() {
    let Ok(url) = std::env::var("KLAMS_URL") else {
        eprintln!("KLAMS_URL unset, skipping live POST");
        return;
    };
    let token = std::env::var("KLAMS_TOKEN").unwrap_or_else(|_| "dev-token".into());

    let script = handoff_dir().join("examples").join("post-userfact.sh");
    let out = Command::new("sh")
        .arg(&script)
        .env("KLAMS_URL", url)
        .env("KLAMS_TOKEN", token)
        .output()
        .expect("spawn");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "script failed: {stderr}\n{stdout}");
    assert!(
        stdout.contains("\"path\""),
        "expected response to include path field: {stdout}",
    );
    assert!(
        stdout.contains("\"canonical\""),
        "expected canonical write: {stdout}",
    );
}
