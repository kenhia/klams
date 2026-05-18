//! Outbound API response DTOs.

use crate::entities::{Event, Fact};
use crate::search::SearchHit;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
}

/// Generic `{id}` response body returned by fire-and-forget POST
/// endpoints (currently `POST /memory/events`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedId {
    pub id: Uuid,
}
