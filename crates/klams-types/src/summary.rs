//! Summarization records: `EventSummary` (Postgres).
//!
//! Sprint 005 (Phase 4) — see sprints/005-advanced-retrieval/data-model.md §3–§4.
//!
//! Sprint 032 (#335) removed `KnowledgeDigest` / `DigestCluster` and the
//! Qdrant `kind = "digest"` machinery behind them. T038 promised to wire
//! it; nothing ever did, and both live collections held zero digest
//! points. `SummaryMechanism::Llm` is kept so summaries written while
//! the (never-generating) LLM relabel existed still deserialize —
//! nothing produces it now.

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryMechanism {
    Extractive,
    Llm,
}

impl SummaryMechanism {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SummaryMechanism::Extractive => "extractive",
            SummaryMechanism::Llm => "llm",
        }
    }
}

/// Row in the Postgres `summaries` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub id: Uuid,
    pub host: String,
    pub category: String,
    pub day_bucket: Date,
    pub source_count: u32,
    pub source_ids: Vec<Uuid>,
    pub summary_text: String,
    pub mechanism: SummaryMechanism,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub invalidated_at: Option<OffsetDateTime>,
}
