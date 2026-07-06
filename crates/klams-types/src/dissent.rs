//! Dissent DTOs and the `FactWriteOutcome` enum returned by the
//! worker for canonical-write paths.
//!
//! See `sprints/002-safety-and-write-ops/data-model.md` for the full
//! state-machine and storage shape.

use crate::entities::{Fact, Source};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DissentStatus {
    Pending,
    Promoted,
    Discarded,
    Orphaned,
}

impl DissentStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DissentStatus::Pending => "pending",
            DissentStatus::Promoted => "promoted",
            DissentStatus::Discarded => "discarded",
            DissentStatus::Orphaned => "orphaned",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dissent {
    pub id: Uuid,
    pub fact_id: Uuid,
    pub proposed_payload: serde_json::Value,
    pub source: Source,
    pub status: DissentStatus,
    #[serde(with = "time::serde::rfc3339")]
    pub submitted_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    pub submission_count: i32,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
    #[serde(default)]
    pub resolved_by_source: Option<Source>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DissentSubmittedResponse {
    pub dissent_id: Uuid,
    pub fact_id: Uuid,
    pub status: DissentStatus,
    /// `true` when this submission matched an existing pending
    /// dissent's `payload_hash` and bumped its `submission_count`
    /// instead of creating a new row.
    pub deduped: bool,
    /// Sprint 003 FR-016: every write-endpoint response carries the
    /// routing path. For this struct it is always
    /// `WritePath::Dissent`; the field is serialised explicitly so
    /// clients can match on `path` without checking the response
    /// type.
    #[serde(default = "crate::responses::default_dissent_path")]
    pub path: crate::responses::WritePath,
}

/// Terminal outcome of a canonical fact write at the worker layer.
///
/// The API layer maps each variant to a distinct HTTP response:
/// `Persisted` → 200, `Dissented` → 202, `VersionConflict` → 409.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FactWriteOutcome {
    Persisted { fact: Fact },
    Dissented { dissent_id: Uuid, fact_id: Uuid },
    VersionConflict { current_version: i32, fact_id: Uuid },
}
