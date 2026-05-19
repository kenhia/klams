//! Outbound API response DTOs.

use crate::dissent::Dissent;
use crate::entities::{Event, Fact};
use crate::search::SearchHit;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Routing path taken by a write request (sprint 003 FR-016).
///
/// Every write-endpoint response includes `path` so the caller can
/// tell at a glance whether the write landed in canonical storage
/// or was diverted to `dissents`. Only `AgentProposal` writes that
/// contradict a higher-trust row ever take the `Dissent` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WritePath {
    Canonical,
    Dissent,
}

impl WritePath {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            WritePath::Canonical => "canonical",
            WritePath::Dissent => "dissent",
        }
    }
}

/// `POST /memory/facts` 200 OK body (sprint 003 — wraps the
/// existing `Fact` entity with the additive `path` field).
///
/// The flatten-attribute keeps every Phase 1/2 field (`id`,
/// `version`, `payload`, …) in its original position; clients that
/// ignored unknown fields keep working.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactWriteResponse {
    #[serde(flatten)]
    pub fact: Fact,
    pub path: WritePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dissent_id: Option<Uuid>,
}

/// `POST /memory/events` 202 ACCEPTED body (sprint 003).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventWriteResponse {
    pub id: Uuid,
    pub path: WritePath,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DissentPage {
    pub items: Vec<Dissent>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactPage {
    pub items: Vec<Fact>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPage {
    pub items: Vec<Event>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub results: Vec<SearchHit>,
    pub total: usize,
    #[serde(default)]
    pub degraded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexKnowledgeResponse {
    pub knowledge_id: Uuid,
    pub deduped: bool,
    /// Added in sprint 003 (FR-016). Knowledge writes never
    /// divert, so this is always `WritePath::Canonical`; the field
    /// is serialised for uniformity with the other write endpoints.
    #[serde(default = "default_canonical_path")]
    pub path: WritePath,
}

/// `POST /memory/knowledge/delete` body (sprint 003 T010b).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeDeleteResponse {
    pub deleted: u64,
    pub path: WritePath,
}

/// Generic `{id}` response body returned by fire-and-forget POST
/// endpoints (currently `POST /memory/events`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedId {
    pub id: Uuid,
}

fn default_canonical_path() -> WritePath {
    WritePath::Canonical
}

pub(crate) fn default_dissent_path() -> WritePath {
    WritePath::Dissent
}
