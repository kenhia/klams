//! `memory_get` MCP tool (sprint 046, WI #1178).
//!
//! The explicit fetch op the compact response contract depends on.
//! `memory_search` returns snippets and locators; this returns the full
//! record behind one locator, in one call.
//!
//! Without it, compact responses would be strictly worse than full
//! text: an agent whose snippet fell short would have to re-run
//! `memory_search` and hope the record came back, paying for a whole
//! ranked list to recover one body. The WI names this as the gap — the
//! REST surface already had per-kind reads, the MCP surface had none.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    tools::McpState,
};
use klams_store::Store;
use klams_types::PublicMemory;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryGetArgs {
    /// The memory id, as returned in a `memory_search` hit's `id`.
    #[schemars(with = "String")]
    pub id: Uuid,
}

/// Fetch one memory of any kind by id.
///
/// # Errors
/// Returns `NOT_FOUND` when no live memory carries that id, or the
/// mapped store error otherwise.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: MemoryGetArgs,
) -> Result<PublicMemory, ErrorEnvelope> {
    match klams_core::fetch::memory_by_id(&state.store, args.id).await {
        Ok(Some(mem)) => Ok(mem),
        // A deleted id and an unknown id are the same answer to a
        // reader; distinguishing them would leak the existence of
        // records the caller cannot see.
        Ok(None) => Err(envelope(
            errors::NOT_FOUND,
            format!("no live memory with id {}", args.id),
        )),
        Err(e) => Err(errors::from_store_error("memory_by_id", &e)),
    }
}
