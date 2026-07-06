//! Summarization records: `EventSummary` (Postgres) and `KnowledgeDigest` (Qdrant).
//!
//! Sprint 005 (Phase 4) — see sprints/005-advanced-retrieval/data-model.md §3–§4.

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

/// Cluster definition for a knowledge digest. Stored in the
/// Qdrant payload alongside the embedded summary text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestCluster {
    pub repo: String,
    pub file_prefix: String,
}

/// Logical view of a `kind=digest` Qdrant point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDigest {
    pub id: Uuid,
    pub text: String,
    pub mechanism: SummaryMechanism,
    pub source_ids: Vec<Uuid>,
    pub source_count: u32,
    pub cluster: DigestCluster,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub invalidated_at: Option<OffsetDateTime>,
}
