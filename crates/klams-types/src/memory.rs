//! Public memory projection (sprint 007).
//!
//! [`PublicMemory`] is the only shape returned by MCP tools and the
//! REST author endpoints. The internal `Fact` / `Event` /
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
        /// Sprint 023 (#409): the host the source file lives on, so a
        /// retrieval result is a fully-qualified `(host, source_path)` an
        /// agent can act on without an extra lookup.
        #[serde(skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        /// Sprint 026 (#641): the chunk's content hash. Clients and the
        /// retrieval eval assert the no-duplicates invariant on this —
        /// it is the key query-time collapse groups by.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_hash: Option<String>,
        /// Sprint 026 (#641): heading breadcrumb the chunker prepended
        /// (scanner v2, sprint 022). Written to the payload since 022 but
        /// never projected until now; lets a client strip the breadcrumb
        /// back off the text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading_path: Option<String>,
        /// Sprint 026 (#641): source language the chunker detected.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        /// Sprint 026 (#641): 0-based index of this chunk within its
        /// source file — lets a client judge fragment-ness and reassemble.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chunk_index: Option<u32>,
        /// Sprint 026 (#641): the duplicates this result absorbed at
        /// query time. Empty on a result that collapsed nothing. Ken's
        /// ruling (WI #641): dedupe keys on **content only**, and the
        /// survivor carries the *list of collapsed points* rather than
        /// merged metadata — lossless, and a caller can reach any copy.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        copies: Vec<KnowledgeCopy>,
        /// Sprint 029 (#638): declared volatility (`"stable"` /
        /// `"volatile"`). Volatile memories are age-demoted in ranking;
        /// undeclared memories never decay.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        volatility: Option<String>,
        /// Sprint 029 (#638): the memory this record replaced via
        /// `memory_supersede` — the inspectable trail back.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<Uuid>,
        /// Sprint 029 (#638): the memory that replaced this one. Only
        /// ever present on superseded (hidden) records, i.e. on the
        /// admin surfaces — never in live search results.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        superseded_by: Option<Uuid>,
    },
    Event {
        category: String,
        payload: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_id: Option<Uuid>,
    },
}

impl PublicMemoryContent {
    /// Project a [`KnowledgeItem`] to its public content body.
    ///
    /// Sprint 026 (#641): this mapping used to be hand-rolled at each
    /// read path (the MCP projection layer plus two sites in
    /// `klams-store::composite`), which is exactly why `heading_path` /
    /// `language` / `chunk_index` were stored since sprint 022 and
    /// projected by nobody. Centralizing it here means a field added to
    /// the wire shape reaches every read path or fails to compile.
    ///
    /// `copies` starts empty; only the search paths that run query-time
    /// collapse populate it (see `klams_core::dedupe`).
    #[must_use]
    pub fn knowledge_from(item: &crate::KnowledgeItem) -> Self {
        Self::Knowledge {
            text: item.text.clone(),
            source_path: item.file.clone(),
            repo: item.repo.clone(),
            host: item.machine.clone(),
            content_hash: Some(item.content_hash.clone()),
            heading_path: item.heading_path.clone(),
            language: item.language.clone(),
            chunk_index: item.chunk_index,
            copies: Vec::new(),
            volatility: item.volatility.clone(),
            supersedes: item.supersedes,
            superseded_by: item.superseded_by,
        }
    }

    /// The content hash, when this is a knowledge body. The key
    /// query-time duplicate collapse groups by (sprint 026, #641).
    #[must_use]
    pub fn content_hash(&self) -> Option<&str> {
        match self {
            Self::Knowledge { content_hash, .. } => content_hash.as_deref(),
            _ => None,
        }
    }

    /// Record the duplicates this result absorbed at query time
    /// (sprint 026, #641). No-op for fact/event bodies, which are never
    /// collapsed.
    pub fn set_copies(&mut self, absorbed: Vec<KnowledgeCopy>) {
        if let Self::Knowledge { copies, .. } = self {
            *copies = absorbed;
        }
    }
}

/// One duplicate absorbed by a surviving search result (sprint 026,
/// WI #641). klams stores the same chunk once per host (sprint 023 made
/// ingest dedupe host-scoped so per-host delete works), so ~44% of the
/// corpus is duplicate content and half of a typical result page was
/// wasted on it. Query-time collapse keeps the best-ranked copy and
/// records the rest here, so nothing is lost — a caller that needs the
/// copy on a particular host can address it by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeCopy {
    /// Point id of the collapsed duplicate.
    pub id: Uuid,
    /// Host the collapsed copy's source file lives on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Source path of the collapsed copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
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
    /// Cross-source **fused** relevance score (RRF, sprint 024 #328).
    /// Comparable across kinds and monotonic with the result order —
    /// this is the ranking key. For raw per-source match quality (Qdrant
    /// cosine / Postgres `ts_rank`), see [`Self::raw_score`].
    pub score: f32,
    /// 0-based rank the hit held **within its own source's** result
    /// list, before cross-source fusion. The global rank is this
    /// hit's index in the returned array.
    pub source_rank: u32,
    /// Raw per-source relevance the hit had before fusion (Qdrant cosine
    /// for knowledge, Postgres `ts_rank` for fact/event) — the match
    /// quality signal klams-mind's identifier-heavy eval judges (sprint
    /// 024 #332). `None` only if unavailable.
    #[serde(default)]
    pub raw_score: Option<f32>,
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
                host: None,
                content_hash: None,
                heading_path: None,
                language: None,
                chunk_index: None,
                copies: Vec::new(),
                volatility: None,
                supersedes: None,
                superseded_by: None,
            },
            tags: vec![],
            author: PublicAuthorRef {
                id: None,
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

    /// Sprint 023 (#409): knowledge results carry the host when known,
    /// so an agent gets a fully-qualified `(host, source_path)`. Omitted
    /// from the wire when absent (back-compat / agent-authored writes).
    #[test]
    fn knowledge_host_serializes_when_present_and_omits_when_absent() {
        let with_host = PublicMemoryContent::Knowledge {
            text: "t".into(),
            source_path: Some("/home/ken/src/x.rs".into()),
            repo: Some("src".into()),
            host: Some("kai".into()),
            content_hash: None,
            heading_path: None,
            language: None,
            chunk_index: None,
            copies: Vec::new(),
            volatility: None,
            supersedes: None,
            superseded_by: None,
        };
        let j = serde_json::to_value(&with_host).unwrap();
        assert_eq!(j["host"], "kai");

        let without = PublicMemoryContent::Knowledge {
            text: "t".into(),
            source_path: None,
            repo: None,
            host: None,
            content_hash: None,
            heading_path: None,
            language: None,
            chunk_index: None,
            copies: Vec::new(),
            volatility: None,
            supersedes: None,
            superseded_by: None,
        };
        let j2 = serde_json::to_value(&without).unwrap();
        assert!(j2.get("host").is_none(), "host must be omitted when None");
    }

    /// Sprint 026 (#641): the projection additions the dedupe, the eval
    /// suite, and the authz work all need. `knowledge_from` is the single
    /// mapping every read path goes through, so this pins what they carry.
    #[test]
    fn knowledge_projection_carries_hash_chunk_metadata_and_no_copies() {
        let item = crate::KnowledgeItem {
            id: Uuid::nil(),
            text: "body".into(),
            content_hash: "abc123".into(),
            source: crate::Source::AgentProposal,
            tags: vec![],
            repo: Some("klams".into()),
            file: Some("/home/ken/src/klams/README.md".into()),
            machine: Some("kai".into()),
            machines: vec![],
            heading_path: Some("klams > MCP tools".into()),
            language: Some("markdown".into()),
            chunk_index: Some(3),
            volatility: None,
            supersedes: None,
            superseded_by: None,
            confidence: 1.0,
            decay_weight: 1.0,
            use_count: 0,
            last_used_at: None,
            created_at: time::OffsetDateTime::UNIX_EPOCH,
            updated_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let j = serde_json::to_value(PublicMemoryContent::knowledge_from(&item)).unwrap();
        assert_eq!(j["content_hash"], "abc123");
        assert_eq!(j["heading_path"], "klams > MCP tools");
        assert_eq!(j["language"], "markdown");
        assert_eq!(j["chunk_index"], 3);
        assert!(
            j.get("copies").is_none(),
            "a result that collapsed nothing carries no copies array"
        );
    }

    /// The `copies` list is how a collapsed duplicate stays reachable.
    #[test]
    fn collapsed_copies_serialize_with_id_host_and_file() {
        let mut content = PublicMemoryContent::Knowledge {
            text: "t".into(),
            source_path: None,
            repo: None,
            host: Some("kai".into()),
            content_hash: Some("abc".into()),
            heading_path: None,
            language: None,
            chunk_index: None,
            copies: Vec::new(),
            volatility: None,
            supersedes: None,
            superseded_by: None,
        };
        content.set_copies(vec![KnowledgeCopy {
            id: Uuid::nil(),
            host: Some("kubs0".into()),
            file: Some("/home/ken/src/x.md".into()),
        }]);
        let j = serde_json::to_value(&content).unwrap();
        assert_eq!(j["copies"][0]["host"], "kubs0");
        assert_eq!(j["copies"][0]["file"], "/home/ken/src/x.md");
        assert_eq!(j["copies"][0]["id"], Uuid::nil().to_string());
    }

    /// `set_copies` is a no-op on non-knowledge bodies — facts and
    /// events are never collapsed.
    #[test]
    fn set_copies_does_nothing_to_a_fact() {
        let mut content = PublicMemoryContent::Fact {
            fact_type: "EnvFact".into(),
            payload: serde_json::json!({}),
        };
        content.set_copies(vec![KnowledgeCopy {
            id: Uuid::nil(),
            host: None,
            file: None,
        }]);
        let j = serde_json::to_value(&content).unwrap();
        assert!(j.get("copies").is_none());
    }

    /// Sprint 016 — the `ScoredMemory` envelope carries `score` +
    /// `source_rank` alongside the (unchanged) memory, and the memory's
    /// `kind` still identifies the source. Round-trips cleanly.
    #[test]
    fn scored_memory_wraps_memory_with_score_and_rank() {
        let hit = ScoredMemory {
            score: 0.5,
            source_rank: 2,
            raw_score: Some(0.87),
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
