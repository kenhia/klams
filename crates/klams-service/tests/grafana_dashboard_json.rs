//! Sprint 006 T049 — `deploy/grafana/klams.json` syntax + series-coverage checks.
//!
//! Sprint 032 (#680): the series contract moved INTO this repo, at
//! `deploy/grafana/SERIES.md`. It used to be read from
//! `$HOME/ansible-k/specs/klams-integration/klams-grafana.md` — a repo
//! inert since 2026-07-05 — and the test **self-skipped when that file
//! was absent**. So on CI, and on any machine without that sibling
//! checkout, the cross-check silently did nothing while reporting
//! green; a check that quietly passes is worse than no check. Sprint
//! 027 discovered this the hard way: adding two panels meant editing a
//! deprecated repo to make a klams test go green.
//!
//! Two directions are now asserted, both unconditionally:
//!
//! 1. every `klams_*` series a dashboard panel queries is documented in
//!    SERIES.md — catches a panel graphing an undocumented series;
//! 2. every `klams_*` series declared in `crates/*/src/**` is documented
//!    in SERIES.md — catches a metric added to the code that nobody
//!    wrote down. This is the direction the old cross-repo check could
//!    never have covered.

use std::collections::HashSet;
use std::path::PathBuf;

fn dashboard_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/grafana/klams.json")
}

fn series_doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/grafana/SERIES.md")
}

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Collect every distinct token matching `klams_[A-Za-z0-9_]+` in `s`.
fn extract_series(s: &str) -> HashSet<String> {
    let bytes = s.as_bytes();
    let mut out = HashSet::new();
    let mut i = 0;
    while i + 6 <= bytes.len() {
        if &bytes[i..i + 6] == b"klams_" {
            let mut j = i + 6;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            out.insert(String::from_utf8_lossy(&bytes[i..j]).into_owned());
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Parse the series-table column from SERIES.md: collect every
/// backtick-wrapped `klams_*` token sitting in the first column of a
/// markdown table row (line starts with `|` and contains `|`).
fn documented_series(md: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for line in md.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('|') {
            continue;
        }
        // First column is between the first and second `|`.
        let after = &trimmed[1..];
        let Some(end) = after.find('|') else {
            continue;
        };
        let cell = after[..end].trim();
        // Strip backticks around the series name.
        let cleaned = cell.trim_matches('`');
        if cleaned.starts_with("klams_") {
            out.insert(cleaned.to_string());
        }
    }
    out
}

#[test]
fn dashboard_json_parses_and_has_panels() {
    let raw = std::fs::read_to_string(dashboard_path()).expect("read dashboard");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("dashboard JSON parses");
    assert_eq!(v["title"], "klams");
    let panels = v["panels"].as_array().expect("panels[] present");
    assert!(
        !panels.is_empty(),
        "dashboard must define at least one panel"
    );
    for p in panels {
        let ds_uid = &p["datasource"]["uid"];
        assert_eq!(
            ds_uid, "prometheus",
            "every panel must pin the `prometheus` datasource UID"
        );
        let targets = p["targets"].as_array().expect("targets[]");
        assert!(!targets.is_empty(), "panel {} has no targets", p["title"]);
        for t in targets {
            assert!(
                t["expr"].as_str().is_some(),
                "target on panel {} is missing expr",
                p["title"]
            );
        }
    }
}

/// Every `"klams_*"` string literal declared under `crates/*/src/`.
/// Names ending in `_test` are fixtures, not exposition.
fn series_declared_in_source(dir: &std::path::Path, out: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            // Only `src/` is exposition; `tests/` holds fixtures that
            // name series they do not emit (this file among them).
            if entry.file_name() == "tests" || entry.file_name() == "benches" {
                continue;
            }
            series_declared_in_source(&p, out);
            continue;
        }
        if p.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        for quoted in text.split('"').skip(1).step_by(2) {
            if quoted.len() > "klams_".len()
                && quoted.starts_with("klams_")
                && !quoted.ends_with("_test")
                && quoted
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                out.insert(quoted.to_string());
            }
        }
    }
}

fn documented() -> HashSet<String> {
    let path = series_doc_path();
    let md =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let known = documented_series(&md);
    assert!(
        !known.is_empty(),
        "could not parse any klams_* series from {} — check the markdown table format",
        path.display()
    );
    known
}

#[test]
fn every_panel_series_is_documented() {
    let dashboard = std::fs::read_to_string(dashboard_path()).expect("read dashboard");
    let v: serde_json::Value = serde_json::from_str(&dashboard).expect("dashboard JSON parses");
    let known = documented();

    let mut referenced: HashSet<String> = HashSet::new();
    for panel in v["panels"].as_array().unwrap() {
        for target in panel["targets"].as_array().unwrap() {
            if let Some(expr) = target["expr"].as_str() {
                referenced.extend(extract_series(expr));
            }
        }
    }
    assert!(
        !referenced.is_empty(),
        "no klams_* series referenced in any panel expr — dashboard authored wrong?"
    );

    let mut unknown: Vec<&String> = referenced.difference(&known).collect();
    unknown.sort();
    assert!(
        unknown.is_empty(),
        "dashboard panels query series not documented in deploy/grafana/SERIES.md: {unknown:?}"
    );
}

#[test]
fn every_source_declared_series_is_documented() {
    let known = documented();
    let mut declared = HashSet::new();
    series_declared_in_source(&crates_dir(), &mut declared);
    assert!(
        declared.len() > 20,
        "expected to find the service's metric constants, found {}",
        declared.len()
    );

    let mut undocumented: Vec<&String> = declared.difference(&known).collect();
    undocumented.sort();
    assert!(
        undocumented.is_empty(),
        "series declared in code but missing from deploy/grafana/SERIES.md: {undocumented:?}"
    );
}

#[test]
fn extract_series_handles_typical_promql() {
    let s = "sum by (route) (rate(klams_http_requests_total{method!=\"GET\"}[5m])) + klams_event_queue_depth";
    let got = extract_series(s);
    assert!(got.contains("klams_http_requests_total"));
    assert!(got.contains("klams_event_queue_depth"));
    assert_eq!(got.len(), 2);
}
