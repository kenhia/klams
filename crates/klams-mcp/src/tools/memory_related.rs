//! `memory_related` MCP tool (sprint 007 T038, US2).
//!
//! Given the id of an existing knowledge memory, returns the nearest
//! neighbours in vector space (excluding the seed point itself). Only
//! knowledge ids are supported — facts and events have no vector
//! representation. Soft-deleted points are excluded by the underlying
//! `search_knowledge` filter.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    projection,
    tools::McpState,
};
use klams_types::{PublicAuthorRef, PublicMemory};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

const DEFAULT_TOP_K: u32 = 5;
const MAX_TOP_K: u32 = 50;

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
pub async fn run(
    state: &McpState,
    args: MemoryRelatedArgs,
) -> Result<Vec<PublicMemory>, ErrorEnvelope> {
    let top_k = args.top_k.unwrap_or(DEFAULT_TOP_K);
    if top_k == 0 || top_k > MAX_TOP_K {
        return Err(envelope(
            errors::INVALID_TOP_K,
            format!("top_k must be 1..={MAX_TOP_K}"),
        ));
    }
    if args.id.is_nil() {
        return Err(envelope(errors::NOT_FOUND, "id is nil"));
    }

    let vector = state
        .store
        .qdrant
        .get_point_vector(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_point_vector: {e}")))?
        .ok_or_else(|| {
            envelope(
                errors::NOT_FOUND,
                format!("knowledge point {} not found", args.id),
            )
        })?;

    // Request one extra so we can drop the seed point if Qdrant
    // returns it (similarity 1.0 to itself).
    let raw = state
        .store
        .qdrant
        .search_knowledge(vector, top_k + 1)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("search_knowledge: {e}")))?;
    let mut hits: Vec<_> = raw.into_iter().filter(|(it, _)| it.id != args.id).collect();
    hits.truncate(top_k as usize);

    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<Uuid> = hits.iter().map(|(it, _)| it.id).collect();
    let author_map = state
        .store
        .qdrant
        .knowledge_authors_by_ids(&ids)
        .await
        .unwrap_or_default();
    let mut wanted: Vec<Uuid> = author_map.values().copied().collect();
    wanted.sort();
    wanted.dedup();
    let mut authors: HashMap<Uuid, PublicAuthorRef> = HashMap::with_capacity(wanted.len());
    for aid in wanted {
        if let Ok(Some(a)) = state.store.postgres.get_author_by_id(aid).await {
            authors.insert(aid, PublicAuthorRef::from_record(&a));
        }
    }

    let mut out = Vec::with_capacity(hits.len());
    for (item, _score) in hits {
        let author_ref = author_map
            .get(&item.id)
            .and_then(|aid| authors.get(aid))
            .cloned()
            .unwrap_or_else(PublicAuthorRef::unknown);
        out.push(projection::project_knowledge(&item, author_ref));
    }
    Ok(out)
}
