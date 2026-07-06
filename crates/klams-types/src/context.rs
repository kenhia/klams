//! `POST /memory/context` request and response shapes.
//!
//! Sprint 005 (Phase 4) — see sprints/005-advanced-retrieval/data-model.md §1–§2.

use crate::entities::{FactType, Source};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    pub query: String,
    pub token_budget: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<RetrievalFilters>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrievalFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<FactType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub since: Option<OffsetDateTime>,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub until: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBundle {
    pub facts: Vec<ContextItem>,
    pub knowledge: Vec<ContextItem>,
    pub events: Vec<ContextItem>,
    pub total_spent: u32,
    pub truncated: bool,
    pub token_encoder: TokenEncoderId,
    pub sections: BTreeMap<String, SectionMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextItem {
    pub kind: ItemKind,
    pub id: Uuid,
    pub score: f32,
    pub tokens: u32,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionMeta {
    pub count: u32,
    pub tokens_spent: u32,
    pub source: SectionSource,
    pub status: SectionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionSource {
    Raw,
    Summary,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionStatus {
    Ok,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Raw,
    Summary,
    Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenEncoderId {
    Cl100kBase,
    CharsDiv4,
}

impl TokenEncoderId {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TokenEncoderId::Cl100kBase => "cl100k_base",
            TokenEncoderId::CharsDiv4 => "chars_div4",
        }
    }
}
