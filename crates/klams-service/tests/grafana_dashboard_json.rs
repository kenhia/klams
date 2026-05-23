//! Sprint 006 T049 — `deploy/grafana/klams.json` syntax + series-coverage smoke test.
//!
//! Parses the dashboard JSON, walks every `panels[].targets[].expr`,
//! extracts the `klams_*` series each `PromQL` references, and asserts
//! the union is a subset of the series table published in the
//! ansible-k handoff `klams-grafana.md`. If the handoff file is not
//! reachable (e.g. CI without ~/ansible-k cloned), the test prints a
//! one-line skip and passes — drift detection happens locally and on
//! the kubsdb deployer.

use std::collections::HashSet;
use std::path::PathBuf;

fn dashboard_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/grafana/klams.json")
}

fn handoff_path() -> PathBuf {
    if let Ok(p) = std::env::var("KLAMS_GRAFANA_HANDOFF") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join("ansible-k/specs/klams-integration/klams-grafana.md")
}

/// Collect every distinct token matching `klams_[A-Za-z0-9_]+` in `s`.
fn extract_series(s: &str) -> HashSet<String> {
    let bytes = s.as_bytes();
    let mut out = HashSet::new();
    let mut i = 0;
    while i + 6 <= bytes.len() {
        if &bytes[i..i + 6] == b"klams_" {
            let mut j = i + 6;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_')
            {
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

/// Parse the series-table column from the handoff doc: collect every
/// backtick-wrapped `klams_*` token sitting in the first column of a
/// markdown table row (line starts with `|` and contains `|`).
fn handoff_series(md: &str) -> HashSet<String> {
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
    assert!(!panels.is_empty(), "dashboard must define at least one panel");
    for p in panels {
        let ds_uid = &p["datasource"]["uid"];
        assert_eq!(
            ds_uid, "prometheus-default",
            "every panel must pin the prometheus-default datasource UID"
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

#[test]
fn every_panel_series_appears_in_handoff_table() {
    let dashboard = std::fs::read_to_string(dashboard_path()).expect("read dashboard");
    let v: serde_json::Value = serde_json::from_str(&dashboard).expect("dashboard JSON parses");

    let handoff = handoff_path();
    if !handoff.exists() {
        eprintln!(
            "skipping handoff cross-check: {} not present (set KLAMS_GRAFANA_HANDOFF to override)",
            handoff.display()
        );
        return;
    }
    let md = std::fs::read_to_string(&handoff).expect("read handoff");
    let known = handoff_series(&md);
    assert!(
        !known.is_empty(),
        "could not parse any klams_* series from {} — check the markdown table format",
        handoff.display()
    );

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

    let unknown: Vec<&String> = referenced.difference(&known).collect();
    assert!(
        unknown.is_empty(),
        "dashboard references series not listed in the handoff table: {unknown:?}\n  handoff = {}\n  known = {known:?}",
        handoff.display()
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
