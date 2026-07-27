//! Sprint 032 (#681) — every YAML file in the repo must load under a
//! strict parser.
//!
//! `sprints/002-safety-and-write-ops/contracts/openapi.yaml` is cited as
//! the REST wire contract, and it had been unparseable since it was
//! written: an unquoted `description` containing a literal `` `degraded:
//! true` `` ends a plain scalar mid-line. Nothing validated it, so any
//! tooling that wanted to generate a client, lint the spec, or diff it
//! against the implementation hit the error immediately and no one
//! found out. `sprints/001` carried the identical copy-pasted line —
//! which is the argument for checking every file rather than fixing the
//! one that was reported.
//!
//! Deliberately NOT a self-skipping test. It walks the repo with no
//! external tool and no env var, so it either runs or fails; a
//! cross-check that quietly does nothing is worse than none (the same
//! lesson as #680).

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("repo root")
}

/// Directories that are not ours to police: build output, vendored
/// dependencies, and the git object store.
fn is_skipped_dir(name: &str) -> bool {
    matches!(name, "target" | "node_modules" | ".git")
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !is_skipped_dir(&name) {
                collect_yaml(&p, out);
            }
            continue;
        }
        // pnpm-lock.yaml is generated; its shape is pnpm's problem.
        if name == "pnpm-lock.yaml" {
            continue;
        }
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(ext, "yaml" | "yml") {
            out.push(p);
        }
    }
}

#[test]
fn every_yaml_file_loads_under_a_strict_parser() {
    let root = repo_root();
    let mut files = Vec::new();
    collect_yaml(&root, &mut files);
    files.sort();
    assert!(
        files.len() >= 5,
        "expected to find the repo's YAML files, found {}",
        files.len()
    );

    let mut failures = Vec::new();
    for f in &files {
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{}: unreadable: {e}", f.display()));
                continue;
            }
        };
        if let Err(e) = yaml_rust2::YamlLoader::load_from_str(&text) {
            let rel = f.strip_prefix(&root).unwrap_or(f);
            failures.push(format!("{}: {e}", rel.display()));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} YAML file(s) failed to parse:\n  {}",
        failures.len(),
        files.len(),
        failures.join("\n  ")
    );
}
