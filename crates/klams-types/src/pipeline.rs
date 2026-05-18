//! The `MemoryWrite` pipeline enum and its variants.
//!
//! Constructed by API handlers and drained by worker tasks.

use crate::entities::{FactType, Source};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum MemoryWrite {
    UpsertFact(UpsertFact),
    AppendEvent(AppendEvent),
    IndexKnowledge(IndexKnowledge),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertFact {
    pub fact_type: FactType,
    pub payload: serde_json::Value,
    pub source: Source,
    pub explicit_id: Option<Uuid>,
    /// Optimistic concurrency token. `Some(0)` indicates the caller
    /// expects this write to create a brand-new canonical fact;
    /// `Some(n)` for `n > 0` asserts the stored row is at version
    /// `n` before the upsert proceeds. `None` is rejected by US1
    /// validation when a canonical fact already exists (FR-008 / A1).
    #[serde(default)]
    pub expected_version: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEvent {
    pub id: Uuid,
    pub task_id: Option<Uuid>,
    pub category: String,
    pub payload: serde_json::Value,
    pub source: Source,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexKnowledge {
    pub id: Uuid,
    pub text: String,
    pub content_hash: String,
    pub source: Source,
    pub tags: Vec<String>,
    pub repo: Option<String>,
    pub file: Option<String>,
    pub machine: Option<String>,
}
