//! Shared DTOs and pipeline types for the klams memory service.
//!
//! Wire formats here are the source of truth; the `OpenAPI` contract is
//! kept aligned manually. See `specs/001-initial-mvp/data-model.md`
//! and `specs/001-initial-mvp/contracts/openapi.yaml`.

pub mod config;
pub mod context;
pub mod decay;
pub mod dissent;
pub mod entities;
pub mod error;
pub mod hash;
pub mod health;
pub mod maintenance;
pub mod pipeline;
pub mod requests;
pub mod responses;
pub mod retrieval;
pub mod search;
pub mod summary;
pub mod validation;

pub use config::{BackupConfig, BackupConfigError, SameDayStrategy, WindowStartUtc};
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
pub use pipeline::{AppendEvent, IndexKnowledge, MemoryWrite, UpsertFact};
pub use requests::{
    AppendEventRequest, IndexKnowledgeRequest, ListDissentsParams, ListEventsParams,
    ListFactsParams, SearchRequest, UpsertFactRequest,
};
pub use responses::{
    AcceptedId, DissentPage, EventPage, EventWriteResponse, FactPage, FactWriteResponse,
    IndexKnowledgeResponse, KnowledgeDeleteResponse, SearchResults, WritePath,
};
pub use retrieval::{FusionStrategy, HybridQueryPlan, RetrievalSource, WeightedNorm};
pub use search::{SearchHit, SearchType};
pub use summary::{DigestCluster, EventSummary, KnowledgeDigest, SummaryMechanism};
pub use validation::{ErrorDetail, ValidationError, ValidationResult};
