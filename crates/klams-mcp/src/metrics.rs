//! Prometheus metrics for MCP traffic (sprint 007 T022 + R-010).
//!
//! Three counters, all using the existing `metrics` facade so they
//! flow through the same Prometheus registry that backs `/metrics`.
//!
//! **Cardinality discipline** (R-010):
//! - `author_id` MUST NOT appear as a label (would explode cardinality).
//! - The search counter MUST NOT carry the request's `kinds` filter as
//!   a label (per-call set, not bounded).
//! - `model` is a bounded enumeration (small set of supported model ids)
//!   and IS allowed as a label.
//! - `agent_name` is bounded by operator policy (small registered set).

use metrics::counter;

/// Memory kind label for `klams_mcp_writes_total`.
pub const KIND_FACT: &str = "fact";
pub const KIND_KNOWLEDGE: &str = "knowledge";
pub const KIND_EVENT: &str = "event";

/// Delete-mode label for `klams_mcp_deletes_total`.
pub const MODE_SOFT: &str = "soft";
pub const MODE_RESTORED: &str = "restored";
pub const MODE_HARD: &str = "hard";

/// Bump the writes counter on a successful `memory_add` /
/// `memory_append_event`.
pub fn record_write(agent_name: &str, model: Option<&str>, kind: &'static str) {
    counter!(
        "klams_mcp_writes_total",
        "agent_name" => agent_name.to_string(),
        "model" => model.unwrap_or("unknown").to_string(),
        "kind" => kind,
    )
    .increment(1);
}

/// Bump the deletes counter for soft / restored / hard transitions.
pub fn record_delete(agent_name: &str, model: Option<&str>, mode: &'static str) {
    counter!(
        "klams_mcp_deletes_total",
        "agent_name" => agent_name.to_string(),
        "model" => model.unwrap_or("unknown").to_string(),
        "mode" => mode,
    )
    .increment(1);
}

/// Bump the search counter on every `memory_search` invocation
/// regardless of the per-call `kinds` filter.
pub fn record_search(agent_name: &str, model: Option<&str>) {
    counter!(
        "klams_mcp_search_total",
        "agent_name" => agent_name.to_string(),
        "model" => model.unwrap_or("unknown").to_string(),
    )
    .increment(1);
}
