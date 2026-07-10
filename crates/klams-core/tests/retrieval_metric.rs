//! Sprint 020 (WI #63) — the shared retrieval-latency histogram.
//!
//! `klams_retrieval_duration_seconds{op, transport}` is the one
//! histogram both the REST handlers (`/memory/search`,
//! `/memory/context`) and the MCP `memory_search` tool record into,
//! so the Grafana "Search + context latency" panel lights up wherever
//! the traffic actually flows (the pre-020 panel watched REST-only
//! axum series that never see traffic — WI #63).
//!
//! Sole test in this file: it installs the process-global Prometheus
//! recorder, so it must not share a test binary with other
//! recorder-installing tests.

use metrics_exporter_prometheus::PrometheusBuilder;

#[test]
fn retrieval_guard_records_op_and_transport_labels() {
    let handle = PrometheusBuilder::new()
        .install_recorder()
        .expect("install recorder");
    klams_core::metrics::describe();

    drop(klams_core::metrics::LatencyGuard::retrieval(
        "search", "mcp",
    ));
    drop(klams_core::metrics::LatencyGuard::retrieval(
        "context", "rest",
    ));

    let rendered = handle.render();
    assert!(
        rendered.contains("klams_retrieval_duration_seconds"),
        "histogram missing from /metrics:\n{rendered}"
    );
    for series in [
        r#"op="search",transport="mcp""#,
        r#"op="context",transport="rest""#,
    ] {
        assert!(
            rendered.contains(series),
            "expected labeled series {series} in:\n{rendered}"
        );
    }
}
