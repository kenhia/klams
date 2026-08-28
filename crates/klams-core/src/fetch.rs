//! Fetch one memory by id (sprint 046, WI #1178).
//!
//! The other half of the compact response contract. Compact search
//! results are only a saving if the follow-up read is **one call** —
//! without a by-id fetch on the MCP surface, an agent that needed the
//! full text of a hit had to run `memory_search` again and hope, which
//! costs more than the full text would have. The WI is explicit that
//! this is the gap: the REST side already had per-kind reads, the MCP
//! side had none.
//!
//! Composed from primitives that already exist rather than new SQL, so
//! it works for every `Store` impl — including `CompositeStore`, where
//! knowledge lives in Qdrant and facts/events in Postgres.

use crate::projection::{project_event, project_fact, project_knowledge};
use klams_store::{Store, StoreError};
use klams_types::{PublicAuthorRef, PublicMemory};
use std::sync::Arc;
use uuid::Uuid;

/// Fetch a live memory of any kind by id.
///
/// Returns `Ok(None)` when no live memory carries that id — a deleted
/// or unknown id are the same answer to a reader, and distinguishing
/// them would leak the existence of records the caller cannot see.
///
/// # Errors
/// Propagates the first backend error. A kind that is merely *absent*
/// is not an error; a kind that fails to answer is.
pub async fn memory_by_id<S: Store>(
    store: &Arc<S>,
    id: Uuid,
) -> Result<Option<PublicMemory>, StoreError> {
    // Knowledge first: it is by far the largest stratum, so it is the
    // likeliest hit and the cheapest expected path.
    if let Some(item) = store.get_knowledge(id).await? {
        let author = knowledge_author(store, id).await;
        return Ok(Some(project_knowledge(&item, author)));
    }

    let facts = store.fetch_facts_with_authors(&[id]).await?;
    if let Some((fact, author)) = facts.first() {
        return Ok(Some(project_fact(
            fact,
            PublicAuthorRef::from_record(author),
        )));
    }

    let events = store.fetch_events_with_authors(&[id]).await?;
    if let Some((event, author)) = events.first() {
        return Ok(Some(project_event(
            event,
            PublicAuthorRef::from_record(author),
        )));
    }

    Ok(None)
}

/// Resolve a knowledge item's author, degrading to `unknown` rather
/// than failing the fetch: an unresolvable author is a worse answer
/// than no answer only if the caller wanted the author, and the text is
/// what they asked for.
async fn knowledge_author<S: Store>(store: &Arc<S>, id: Uuid) -> PublicAuthorRef {
    let Ok(map) = store.knowledge_authors_by_ids(&[id]).await else {
        return PublicAuthorRef::unknown();
    };
    let Some(author_id) = map.get(&id).copied() else {
        return PublicAuthorRef::unknown();
    };
    match store.get_author_by_id(author_id).await {
        Ok(Some(a)) => PublicAuthorRef::from_record(&a),
        _ => PublicAuthorRef::unknown(),
    }
}
