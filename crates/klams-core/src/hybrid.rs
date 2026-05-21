//! Hybrid retrieval adapter.
//!
//! Sprint 005 (Phase 4) — T019. `StoreHybridAdapter` implements
//! [`klams_store::HybridStore`] on top of any [`klams_store::Store`].
//! In US1 this is "vector-only" in the sense that no fusion is
//! performed across sources: the `Vector` source returns knowledge
//! hits from Qdrant; the `Fts` source returns fact + event text
//! hits from Postgres; the `MetadataOnly` source returns an empty
//! vector (US2 will replace it with the real metadata-filter path).
//!
//! The adapter is generic over `Store` so handlers can wrap their
//! existing `Arc<S>` without any plumbing changes.

use async_trait::async_trait;
use klams_store::{HybridStore, RankedRow, Store, StoreResult, TextHit};
use klams_types::{ItemKind, RetrievalFilters, RetrievalSource};
use std::sync::Arc;

/// Wraps a [`Store`] and exposes the [`HybridStore`] trait.
///
/// Cheap to clone.
#[derive(Clone)]
pub struct StoreHybridAdapter<S: Store> {
    store: Arc<S>,
}

impl<S: Store> StoreHybridAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

impl<S: Store> std::fmt::Debug for StoreHybridAdapter<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreHybridAdapter")
            .field("store", &"<dyn Store>")
            .finish()
    }
}

#[async_trait]
impl<S: Store> HybridStore for StoreHybridAdapter<S> {
    async fn retrieve(
        &self,
        source: RetrievalSource,
        query: &str,
        _filters: &RetrievalFilters,
        per_source_top_k: u32,
    ) -> StoreResult<Vec<RankedRow>> {
        match source {
            RetrievalSource::Vector => {
                let vec = self.store.embed_query(query).await?;
                let hits = self.store.search_knowledge(vec, per_source_top_k).await?;
                let max = hits.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
                let rows = hits
                    .into_iter()
                    .map(|(item, score)| RankedRow {
                        source: RetrievalSource::Vector,
                        id: item.id,
                        score: normalize(score, max),
                        payload: serde_json::json!({
                            "kind": ItemKind::Raw,
                            "section": "knowledge",
                            "text": item.text,
                            "file": item.file,
                            "tags": item.tags,
                            "repo": item.repo,
                        }),
                    })
                    .collect();
                Ok(rows)
            }
            RetrievalSource::Fts => {
                let (facts, events) = self.store.search_text(query, per_source_top_k).await?;
                let mut rows = Vec::with_capacity(facts.len() + events.len());
                rows.extend(into_rows(facts, "facts"));
                rows.extend(into_rows(events, "events"));
                Ok(rows)
            }
            RetrievalSource::MetadataOnly => {
                // Placeholder for US2 — sprint 005 T028 will populate
                // this with the real metadata-filter scan.
                Ok(Vec::new())
            }
        }
    }
}

fn into_rows(hits: Vec<TextHit>, section: &'static str) -> Vec<RankedRow> {
    let max = hits.iter().map(|h| h.score).fold(0.0_f32, f32::max);
    hits.into_iter()
        .map(|h| {
            let mut payload = h.payload;
            if let serde_json::Value::Object(ref mut m) = payload {
                m.insert("section".into(), serde_json::Value::String(section.into()));
            }
            RankedRow {
                source: RetrievalSource::Fts,
                id: h.id,
                score: normalize(h.score, max),
                payload,
            }
        })
        .collect()
}

fn normalize(score: f32, max: f32) -> f32 {
    if max > 0.0 {
        (score / max).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Behavioural tests live in the `ContextBuilder` tests
    // (`klams_core::context`) because the adapter has no logic
    // beyond delegating to the wrapped `Store`. Trait-shape
    // verification is sufficient here.

    #[test]
    fn normalize_handles_zero_max() {
        assert!((normalize(0.0, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((normalize(0.5, 1.0) - 0.5).abs() < f32::EPSILON);
        assert!((normalize(2.0, 1.0) - 1.0).abs() < f32::EPSILON);
    }
}
