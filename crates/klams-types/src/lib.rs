//! Shared DTOs and pipeline types for the klams memory service.
//!
//! Wire formats here are the source of truth; the `OpenAPI` contract is
//! kept aligned manually. See `sprints/001-initial-mvp/data-model.md`
//! and `sprints/001-initial-mvp/contracts/openapi.yaml`.

pub mod auth;
pub mod author;
pub mod config;
pub mod context;
pub mod decay;
pub mod dissent;
pub mod entities;
pub mod error;
pub mod hash;
pub mod health;
pub mod maintenance;
pub mod memory;
pub mod pipeline;
pub mod requests;
pub mod responses;
pub mod retrieval;
pub mod search;
pub mod summary;
pub mod validation;

/// UUID of the seeded `system` author. Backfilled into every pre-MCP fact
/// and event row by migrations 0006 and 0007.
pub const SYSTEM_AUTHOR_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0001_u128);

/// UUID of the seeded `lost-author` identity (sprint 009, FR-016a).
/// The one-shot re-attribution repair routes rows whose true writer
/// cannot be unambiguously recovered (no provenance, ambiguous
/// provenance, or recovered author deleted) to this author so the
/// unrecoverable bucket stays queryable instead of hiding inside the
/// `system` author's share. Seeded by migration `0008`.
pub const LOST_AUTHOR_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0002_u128);

/// Helper used as a `#[serde(default = "...")]` for pipeline
/// structs' `author_id` field. Provides back-compat for serialized
/// payloads written before sprint 009 added the field.
#[must_use]
pub fn system_author_id() -> uuid::Uuid {
    SYSTEM_AUTHOR_ID
}

pub use auth::{
    validate_agent_name, AgentNameInvalidReason, AuthConfigError, Scope, TokenGrantConfig,
};
pub use author::{AuthorRecord, PublicAuthorRef, RegisterAuthorArgs, RegisterAuthorError};
pub use config::{ApiConfig, BackupConfig, BackupConfigError, SameDayStrategy, WindowStartUtc};
pub use context::{
    ContextBundle, ContextItem, ContextRequest, ItemKind, RetrievalFilters, SectionMeta,
    SectionSource, SectionStatus, TokenEncoderId,
};
pub use decay::DecayConfig;
pub use dissent::{Dissent, DissentStatus, DissentSubmittedResponse, FactWriteOutcome};
pub use entities::{Event, Fact, FactType, KnowledgeItem, Source};
pub use error::ApiError;
pub use hash::canonical_json_hash;
pub use health::{HealthSnapshot, HealthStatus, QueueStatus, SubsystemStatus};
pub use maintenance::{MaintenanceSnapshot, MaintenanceState, RunningSnapshot};
pub use memory::{MemoryKind, PublicMemory, PublicMemoryContent};
pub use pipeline::{AppendEvent, IndexKnowledge, MemoryWrite, UpsertFact};
pub use requests::{
    AppendEventRequest, IndexKnowledgeRequest, ListAuthorMemoriesParams, ListAuthorsParams,
    ListDissentsParams, ListEventsParams, ListFactsParams, ListMemoriesParams, SearchRequest,
    UpsertFactRequest,
};
pub use responses::{
    AcceptedId, AuthorCounts, AuthorMemoriesPage, AuthorMemoryRow, AuthorPage, DissentPage,
    EventPage, EventWriteResponse, FactPage, FactWriteResponse, IndexKnowledgeResponse,
    KnowledgeDeleteResponse, MemoriesPage, MemoryRow, PublicAuthor, SearchResults, WritePath,
};
pub use retrieval::{FusionStrategy, HybridQueryPlan, RetrievalSource, WeightedNorm};
pub use search::{SearchHit, SearchType};
pub use summary::{DigestCluster, EventSummary, KnowledgeDigest, SummaryMechanism};
pub use validation::{ErrorDetail, ValidationError, ValidationResult};
