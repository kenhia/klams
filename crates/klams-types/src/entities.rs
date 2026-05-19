//! Core persisted entities: `Fact`, `Event`, `KnowledgeItem`,
//! plus the supporting enums `FactType` and `Source`.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)] // API-stable JSON tags
pub enum FactType {
    UserFact,
    TaskFact,
    EnvFact,
}

impl FactType {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FactType::UserFact => "UserFact",
            FactType::TaskFact => "TaskFact",
            FactType::EnvFact => "EnvFact",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Source {
    User,
    Controller,
    Task,
    AgentProposal,
}

impl Source {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Source::User => "User",
            Source::Controller => "Controller",
            Source::Task => "Task",
            Source::AgentProposal => "AgentProposal",
        }
    }

    /// Trust rank for this source. Higher = more authoritative.
    /// Single source of truth for both the write dispatcher
    /// (`klams-store::postgres`) and the `PolicyTable` exposed via
    /// `GET /memory/policy` (`klams-core::policy`).
    #[must_use]
    pub fn trust_rank(self) -> u8 {
        match self {
            Source::User => 4,
            Source::Controller => 3,
            Source::Task => 2,
            Source::AgentProposal => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_field_names)] // `fact_type` serialises as `type`
pub struct Fact {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub fact_type: FactType,
    pub payload: serde_json::Value,
    pub version: i32,
    pub source: Source,
    pub confidence: f32,
    pub decay_weight: f32,
    pub use_count: i32,
    /// Count of dissents with status='pending' targeting this fact.
    /// Maintained by Postgres triggers (see migration 0002).
    /// New in sprint 002.
    #[serde(default)]
    pub dissent_count: i32,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Uuid,
    pub task_id: Option<Uuid>,
    pub category: String,
    pub payload: serde_json::Value,
    pub source: Source,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeItem {
    pub id: Uuid,
    pub text: String,
    pub content_hash: String,
    pub source: Source,
    #[serde(default)]
    pub tags: Vec<String>,
    pub repo: Option<String>,
    pub file: Option<String>,
    pub machine: Option<String>,
    pub confidence: f32,
    pub decay_weight: f32,
    pub use_count: i64,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}
