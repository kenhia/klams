//! Hybrid-retrieval query plan and fusion strategy.
//!
//! Sprint 005 (Phase 4) — see sprints/005-advanced-retrieval/data-model.md §6–§7.

use crate::context::RetrievalFilters;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridQueryPlan {
    pub query: String,
    pub filters: RetrievalFilters,
    pub fusion: FusionStrategy,
    pub per_source_top_k: u32,
    pub sources: Vec<RetrievalSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetrievalSource {
    Vector,
    Fts,
    MetadataOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum FusionStrategy {
    Rrf {
        k: u32,
    },
    Weighted {
        vector: f32,
        fts: f32,
        normalization: WeightedNorm,
    },
}

impl FusionStrategy {
    pub const DEFAULT_RRF_K: u32 = 60;

    #[must_use]
    pub fn default_rrf() -> Self {
        FusionStrategy::Rrf {
            k: Self::DEFAULT_RRF_K,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WeightedNorm {
    ZScore,
    MinMax,
}
