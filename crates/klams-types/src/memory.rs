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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_kind_serializes_lowercase() {
        let s = serde_json::to_string(&MemoryKind::Knowledge).unwrap();
        assert_eq!(s, "\"knowledge\"");
    }
}
