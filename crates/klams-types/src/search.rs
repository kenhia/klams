//! Search request/result shapes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchType {
    Fact,
    Event,
    Knowledge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(rename = "type")]
    pub kind: SearchType,
    pub id: Uuid,
    /// Cross-source **fused** relevance (weighted RRF since sprint 036,
    /// #730 — the same value MCP `memory_search` reports). Comparable
    /// across kinds *within one response* and monotonic with the result
    /// order; not a similarity percentage. Before 036 this was a
    /// clamped, adapter-normalized value on a third scale.
    pub score: f32,
    pub preview: String,
    pub payload: serde_json::Value,
    /// Raw per-source relevance before fusion (Qdrant cosine for
    /// knowledge, Postgres `ts_rank` for facts/events) — the
    /// match-quality signal (#332). Additive in sprint 036; absent on
    /// older servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_score: Option<f32>,
    /// 0-based rank the hit held within its own source's result list
    /// before cross-source fusion. Additive in sprint 036.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_rank: Option<u32>,
}
