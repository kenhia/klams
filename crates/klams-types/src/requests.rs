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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub created_after: Option<OffsetDateTime>,
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
