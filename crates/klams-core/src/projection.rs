//! Public projection layer (sprint 007 T020; moved here from
//! `klams-mcp` in sprint 036, #730, so the REST surface can share it).
//!
//! Pure functions that map internal `Fact`/`Event`/`KnowledgeItem`
//! values to the public [`PublicMemory`] shape. Every public-facing
//! response (MCP tool result, REST search hit, or author REST route)
//! MUST go through this module — direct serialization of the
//! internal types would leak `version`, `decay_weight`, `confidence`,
//! `use_count`, `last_used_at`, the raw embedding vector, the internal
//! `source` trust tier, and the soft-deletion bookkeeping columns.

use chrono::{DateTime, Utc};
use klams_types::{Event, Fact, KnowledgeItem, PublicAuthorRef, PublicMemory, PublicMemoryContent};

/// Project a [`Fact`] to the public wire shape. The caller supplies the
/// `author` because it is denormalized from `authors` at the call site
/// (typically via a join on `author_id`).
#[must_use]
pub fn project_fact(fact: &Fact, author: PublicAuthorRef) -> PublicMemory {
    PublicMemory {
        id: fact.id,
        content: PublicMemoryContent::Fact {
            fact_type: fact.fact_type.as_str().to_string(),
            payload: fact.payload.clone(),
        },
        tags: Vec::new(),
        author,
        created_at: offset_to_chrono(fact.created_at),
        updated_at: offset_to_chrono(fact.updated_at),
        deleted_at: None,
        deleted_by_author_id: None,
    }
}

/// Project a [`KnowledgeItem`] to the public wire shape.
#[must_use]
pub fn project_knowledge(item: &KnowledgeItem, author: PublicAuthorRef) -> PublicMemory {
    PublicMemory {
        id: item.id,
        content: PublicMemoryContent::knowledge_from(item),
        tags: item.tags.clone(),
        author,
        created_at: offset_to_chrono(item.created_at),
        updated_at: offset_to_chrono(item.updated_at),
        deleted_at: None,
        deleted_by_author_id: None,
    }
}

/// Project an [`Event`] to the public wire shape.
#[must_use]
pub fn project_event(event: &Event, author: PublicAuthorRef) -> PublicMemory {
    PublicMemory {
        id: event.id,
        content: PublicMemoryContent::Event {
            category: event.category.clone(),
            payload: event.payload.clone(),
            task_id: event.task_id,
        },
        tags: Vec::new(),
        author,
        created_at: offset_to_chrono(event.created_at),
        updated_at: offset_to_chrono(event.created_at),
        deleted_at: None,
        deleted_by_author_id: None,
    }
}

fn offset_to_chrono(ts: time::OffsetDateTime) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_nanos(i64::try_from(ts.unix_timestamp_nanos()).unwrap_or(0))
}
