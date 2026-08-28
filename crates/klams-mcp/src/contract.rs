//! The compact response contract (sprint 046, WI #1178).
//!
//! Ported from khound's `docs/response-contract.md`. khound stopped in
//! August 2026, but the contract is engine-independent and the numbers
//! behind it were measured on a fair suite: klams' full-text
//! `memory_search` cost **4,491 tokens per answered query**; the
//! compact contract cost **1,024–1,476** on the same suite with answers
//! preserved. The token model charges a follow-up read whenever the
//! snippet did not carry the answer, so those are not savings taken out
//! of recall.
//!
//! Two rules govern the shape:
//!
//! 1. **Compact by default.** The agent asks for more; it is not
//!    pushed. Full text is available through `memory_get`, one call.
//! 2. **Typed metadata, uniformly.** Knowledge hits carry their source
//!    locator, facts carry their type, events carry their category.
//!    Fields that do not apply are **omitted, not faked** — an absent
//!    `source_path` on a fact is honest, an empty string would not be.
//!
//! This is also the shape korg already adopted for the same reason:
//! lean rows by default, a focused read when the caller wants the
//! payload.

use klams_core::snippet::match_window;
use klams_types::{MemoryKind, PublicMemory, PublicMemoryContent, ScoredMemory};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One compact search hit: enough to rank, cite and decide on, and a
/// locator (`id`) for the one call that yields the rest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactHit {
    /// Pass to `memory_get` for the full record.
    pub id: Uuid,
    pub kind: MemoryKind,
    /// Match-window excerpt, ≤320 chars. Elisions are marked `…`.
    pub snippet: String,
    /// Fused cross-source relevance (RRF) — the ranking key.
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_score: Option<f32>,
    pub source_rank: u32,
    /// Age of the record in seconds, so freshness is legible without
    /// the caller parsing two timestamps and doing the subtraction.
    pub age_seconds: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub author: String,

    // ---- typed metadata; omitted when it does not apply ----
    /// Knowledge: the file this chunk came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Knowledge: heading breadcrumb, so a snippet from deep in a
    /// document still says where in the document it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<String>,
    /// Knowledge: the memory this record replaced, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Uuid>,
    /// Fact: the fact type.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub fact_type: Option<String>,
    /// Event: the event category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// How many duplicate copies this hit absorbed at query time.
    /// Omitted when it collapsed nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copies: Option<usize>,
}

/// The explicit "here is how you get the rest" pointer.
///
/// Present on every response, not only truncated ones. A field that
/// appears only sometimes is a field nobody learns to read — the same
/// reasoning khound applied to its `backends` block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct More {
    /// The tool that returns a hit's full record.
    pub fetch: String,
    /// True when a snippet was elided, i.e. when `fetch` may yield
    /// something the snippet did not carry.
    pub truncated: bool,
}

/// `memory_search`'s compact response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompactSearchResponse {
    pub hits: Vec<CompactHit>,
    pub more: More,
}

impl CompactSearchResponse {
    /// Build the compact response for a ranked result list.
    #[must_use]
    pub fn build(hits: &[ScoredMemory], query: &str) -> Self {
        let hits: Vec<CompactHit> = hits.iter().map(|h| CompactHit::build(h, query)).collect();
        let truncated = hits.iter().any(|h| h.snippet.contains('…'));
        Self {
            hits,
            more: More {
                fetch: "memory_get".into(),
                truncated,
            },
        }
    }
}

impl CompactHit {
    /// Project one scored hit into the compact shape.
    #[must_use]
    pub fn build(hit: &ScoredMemory, query: &str) -> Self {
        let mem = &hit.memory;
        let mut out = Self {
            id: mem.id,
            kind: mem.kind(),
            snippet: snippet_for(mem, query),
            score: hit.score,
            raw_score: hit.raw_score,
            source_rank: hit.source_rank,
            age_seconds: (chrono::Utc::now() - mem.created_at).num_seconds().max(0),
            tags: mem.tags.clone(),
            author: mem.author.agent_name.clone(),
            source_path: None,
            repo: None,
            host: None,
            heading_path: None,
            supersedes: None,
            fact_type: None,
            category: None,
            copies: None,
        };
        match &mem.content {
            PublicMemoryContent::Knowledge {
                source_path,
                repo,
                host,
                heading_path,
                supersedes,
                copies,
                ..
            } => {
                out.source_path.clone_from(source_path);
                out.repo.clone_from(repo);
                out.host.clone_from(host);
                out.heading_path.clone_from(heading_path);
                out.supersedes = *supersedes;
                if !copies.is_empty() {
                    out.copies = Some(copies.len());
                }
            }
            PublicMemoryContent::Fact { fact_type, .. } => {
                out.fact_type = Some(fact_type.clone());
            }
            PublicMemoryContent::Event { category, .. } => {
                out.category = Some(category.clone());
            }
        }
        out
    }
}

/// The text a snippet is cut from, per kind.
///
/// Knowledge has prose. Facts and events are JSON payloads, and their
/// compact rendering is the payload serialized — the type/category is
/// carried in its own typed field rather than being duplicated into the
/// snippet.
fn snippet_for(mem: &PublicMemory, query: &str) -> String {
    let text = match &mem.content {
        PublicMemoryContent::Knowledge { text, .. } => text.clone(),
        PublicMemoryContent::Fact { payload, .. } | PublicMemoryContent::Event { payload, .. } => {
            serde_json::to_string(payload).unwrap_or_default()
        }
    };
    match_window(&text, query)
}
