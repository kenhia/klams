//! Public memory projection (sprint 007).
//!
//! [`PublicMemory`] is the only shape returned by MCP tools and the
//! viewport REST author endpoints. The internal `Fact` / `Event` /
//! `KnowledgeItem` types carry decay state, trust tiers, and embedding
//! vectors that are deliberately stripped before crossing the public
//! boundary — see `data-model.md` §6.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::author::PublicAuthorRef;

/// Discriminator for the three memory kinds exposed via MCP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Fact,
    Knowledge,
    Event,
}

/// Per-kind body for [`PublicMemory`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PublicMemoryContent {
    Fact {
        #[serde(rename = "type")]
        fact_type: String,
        payload: serde_json::Value,
    },
    Knowledge {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_path: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
    },
    Event {
        category: String,
        payload: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<Uuid>,
    },
}

/// Sanitized wire shape returned by every MCP tool that yields memories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PublicMemory {
    pub id: Uuid,
    #[serde(flatten)]
    pub content: PublicMemoryContent,
    #[serde(default)]
    pub tags: Vec<String>,
    pub author: PublicAuthorRef,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Sprint 008 FR-010 — surfaced only when the row is soft-deleted
    /// and the caller requested `state ∈ {deleted, all}`. Omitted from
    /// the wire shape (and `None` for live rows) by `skip_serializing_if`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    /// Sprint 008 FR-010 — author UUID that performed the soft-delete.
    /// Same gating as [`Self::deleted_at`]. The richer `PublicAuthorRef`
    /// projection is surfaced separately by the wrapper response shape
    /// (see `klams-store::ListMemoriesRow`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_by_author_id: Option<Uuid>,
}

impl PublicMemory {
    /// Discriminator derived from [`PublicMemoryContent`]. The wire
    /// shape exposes a single `kind` field via the internally-tagged
    /// `content`; this accessor lets Rust callers read it without
    /// matching on the enum.
    #[must_use]
    pub fn kind(&self) -> MemoryKind {
        match self.content {
            PublicMemoryContent::Fact { .. } => MemoryKind::Fact,
            PublicMemoryContent::Knowledge { .. } => MemoryKind::Knowledge,
            PublicMemoryContent::Event { .. } => MemoryKind::Event,
        }
    }
}

/// A single scored `memory_search` result (sprint 016). Wraps the
/// [`PublicMemory`] projection with the retrieval metadata the ranked
/// list used to discard, so callers (klams-mind's eval harness) can
/// see *why* a hit ranked where it did.
///
/// Distinct from [`crate::SearchHit`] (the flattened preview/payload
/// shape returned by the REST `/memory/search` endpoint); this one
/// carries the full public projection because the MCP `memory_search`
/// tool contractually returns `PublicMemory`.
///
/// **Score scale caveat**: `score` is the raw per-source relevance
/// score, exposed verbatim and **not** normalized across kinds —
/// knowledge scores are Qdrant cosine similarity (~0..1), fact/event
/// scores are Postgres `ts_rank` (unbounded, typically ≪1). Compare
/// scores only within the same [`PublicMemory::kind`]. The source
/// backend maps 1:1 to that kind, so there is no separate `source_kind`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScoredMemory {
    /// Raw per-source relevance score (see the scale caveat above).
    pub score: f32,
    /// 0-based rank the hit held **within its own source's** result
    /// list, before cross-source fusion. The global rank is this
    /// hit's index in the returned array.
    pub source_rank: u32,
    pub memory: PublicMemory,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_kind_serializes_lowercase() {
        let s = serde_json::to_string(&MemoryKind::Knowledge).unwrap();
        assert_eq!(s, "\"knowledge\"");
    }

    fn sample_memory() -> PublicMemory {
        PublicMemory {
            id: Uuid::nil(),
            content: PublicMemoryContent::Knowledge {
                text: "hello".into(),
                source_path: None,
                repo: None,
            },
            tags: vec![],
            author: PublicAuthorRef {
                agent_name: "a".into(),
                model: None,
                repo: None,
            },
            created_at: DateTime::<Utc>::UNIX_EPOCH,
            updated_at: DateTime::<Utc>::UNIX_EPOCH,
            deleted_at: None,
            deleted_by_author_id: None,
        }
    }

    /// Sprint 016 — the `ScoredMemory` envelope carries `score` +
    /// `source_rank` alongside the (unchanged) memory, and the memory's
    /// `kind` still identifies the source. Round-trips cleanly.
    #[test]
    fn scored_memory_wraps_memory_with_score_and_rank() {
        let hit = ScoredMemory {
            score: 0.5,
            source_rank: 2,
            memory: sample_memory(),
        };
        let v = serde_json::to_value(&hit).unwrap();
        // 0.5 is exactly representable as f32, so JSON equality is safe.
        assert_eq!(v["score"], 0.5);
        assert_eq!(v["source_rank"], 2);
        // The wrapped memory keeps its own shape, including the `kind`
        // discriminator that doubles as the source — no separate field.
        assert_eq!(v["memory"]["kind"], "knowledge");
        assert!(v.get("source_kind").is_none());
        let back: ScoredMemory = serde_json::from_value(v).unwrap();
        assert_eq!(back, hit);
        assert_eq!(back.memory.kind(), MemoryKind::Knowledge);
    }
}
