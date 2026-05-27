//! Inbound API request DTOs.

use crate::entities::{FactType, Source};
use crate::search::SearchType;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertFactRequest {
    #[serde(rename = "type")]
    pub fact_type: FactType,
    pub payload: serde_json::Value,
    pub source: Source,
    #[serde(default)]
    pub explicit_id: Option<Uuid>,
    /// Optimistic concurrency token (see `UpsertFact::expected_version`).
    /// Required by validators when the `(type, payload_hash)` pair
    /// already exists; new facts pass `0`.
    #[serde(default)]
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEventRequest {
    #[serde(default)]
    pub task_id: Option<Uuid>,
    pub category: String,
    pub payload: serde_json::Value,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexKnowledgeRequest {
    pub text: String,
    pub source: Source,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub types: Option<Vec<SearchType>>,
    #[serde(default)]
    pub filters: Option<serde_json::Value>,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
}

fn default_top_k() -> u32 {
    10
}

/// Query parameters for `GET /memory/events`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListEventsParams {
    /// Filters by `events.task_id` column (typed `Uuid`) when parseable
    /// AND by `payload->>'task_id'` (raw string, hits the new
    /// `events_task_id_created_at_idx` from sprint 003). Either match is
    /// returned (logical OR) so both controller-prefixed and column-stored
    /// task ids resolve from one query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Filter by `payload->>'service'` — used by US3 monitor queries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub created_after: Option<OffsetDateTime>,
    /// Alias for `created_after`. When both are set, `created_after` wins.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub since: Option<OffsetDateTime>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub created_before: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query parameters for `GET /memory/facts`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListFactsParams {
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub fact_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query parameters for `GET /memory/dissents`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListDissentsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<uuid::Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query parameters for `GET /v1/authors` (sprint 007).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListAuthorsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query parameters for `GET /v1/authors/{id}/memories` (sprint 007).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListAuthorMemoriesParams {
    /// Comma-separated subset of `fact,knowledge,event`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// `live | deleted | all` (default `live`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Query parameters for `GET /v1/memories` (sprint 008).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListMemoriesParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Comma-separated subset of `fact,knowledge,event`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// `live | deleted | all` (default `live`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Comma-separated UUID list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}
