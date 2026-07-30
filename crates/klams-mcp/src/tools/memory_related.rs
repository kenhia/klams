//! `memory_related` MCP tool (sprint 007 T038, US2).
//!
//! Given the id of an existing knowledge memory, returns the nearest
//! neighbours in vector space (excluding the seed point itself). Only
//! knowledge ids are supported — facts and events have no vector
//! representation.
//!
//! Since sprint 036 (#730) this is a shell over
//! [`klams_core::retrieval::related`], which adds what bare ANN never
//! had: duplicate collapse (a neighbour on several hosts is one result
//! carrying its copies), exclusion of copies of the seed's own content,
//! and the pipeline's live-only guarantee (soft-deleted and superseded
//! points are filtered by the ANN query itself).

use crate::{errors::ErrorEnvelope, tools::McpState};
use klams_core::retrieval;
use klams_store::Store;
use klams_types::PublicMemory;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryRelatedArgs {
    #[schemars(with = "String")]
    pub id: Uuid,
    #[serde(default)]
    pub top_k: Option<u32>,
}

/// Execute `memory_related`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `INVALID_TOP_K`, `NOT_FOUND`,
/// or `INTERNAL_ERROR`.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: MemoryRelatedArgs,
) -> Result<Vec<PublicMemory>, ErrorEnvelope> {
    retrieval::related(&state.store, args.id, args.top_k)
        .await
        .map_err(super::memory_search::map_error)
}
