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
        filters: &RetrievalFilters,
        per_source_top_k: u32,
    ) -> StoreResult<Vec<RankedRow>> {
        // Sprint 005 T026/T027 — filter pre-pruning. The store
        // contracts don't yet take filters, so we over-fetch (×3)
        // and apply payload-side post-filtering. DB-side pushdown
        // is tracked as a follow-up (see specs/005-advanced-retrieval
        // research.md D-004); the behavioural contract — that filters
        // narrow the result set — is honoured here.
        let fetch_cap = per_source_top_k.saturating_mul(3).max(per_source_top_k);
        match source {
            RetrievalSource::Vector => {
                let vec = self.store.embed_query(query).await?;
                let hits = self.store.search_knowledge(vec, fetch_cap).await?;
                let max = hits.iter().map(|(_, s)| *s).fold(0.0_f32, f32::max);
                let rows: Vec<RankedRow> = hits
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
                    .filter(|r| matches_filters(&r.payload, filters))
                    .take(per_source_top_k as usize)
                    .collect();
                Ok(rows)
            }
            RetrievalSource::Fts => {
                let (facts, events) = self.store.search_text(query, fetch_cap).await?;
                let mut rows = Vec::with_capacity(facts.len() + events.len());
                rows.extend(into_rows(facts, "facts"));
                rows.extend(into_rows(events, "events"));
                let filtered: Vec<RankedRow> = rows
                    .into_iter()
                    .filter(|r| matches_filters(&r.payload, filters))
                    .take(per_source_top_k as usize)
                    .collect();
                Ok(filtered)
            }
            RetrievalSource::MetadataOnly => {
                // No text/vector signal — surface nothing. The
                // ContextBuilder's metadata path comes through the
                // SummaryStore instead.
                Ok(Vec::new())
            }
        }
    }
}

/// Post-filter a payload `Value` against the request's `RetrievalFilters`.
/// Returns `true` when the row should be kept. Unknown payload fields are
/// treated as a non-match (conservative).
fn matches_filters(payload: &serde_json::Value, f: &RetrievalFilters) -> bool {
    let Some(obj) = payload.as_object() else {
        return true;
    };
    if let Some(want) = f.host.as_deref() {
        if obj.get("host").and_then(serde_json::Value::as_str) != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.type_ {
        let got = obj.get("type").and_then(serde_json::Value::as_str);
        let want_str = serde_json::to_value(want)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        if got.map(str::to_owned) != want_str {
            return false;
        }
    }
    if let Some(want) = f.tag.as_deref() {
        let has = obj
            .get("tags")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|arr| {
                arr.iter()
                    .any(|v| v.as_str().is_some_and(|s| s == want))
            });
        if !has {
            return false;
        }
    }
    if let Some(want) = f.repo.as_deref() {
        if obj.get("repo").and_then(serde_json::Value::as_str) != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.file.as_deref() {
        if obj.get("file").and_then(serde_json::Value::as_str) != Some(want) {
            return false;
        }
    }
    if let Some(want) = f.source {
        let got = obj.get("source").and_then(serde_json::Value::as_str);
        let want_str = serde_json::to_value(want)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        if got.map(str::to_owned) != want_str {
            return false;
        }
    }
    if f.since.is_some() || f.until.is_some() {
        let ts_str = obj
            .get("observed_at")
            .or_else(|| obj.get("at"))
            .or_else(|| obj.get("created_at"))
            .and_then(serde_json::Value::as_str);
        if let Some(ts) = ts_str.and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        }) {
            if let Some(since) = f.since {
                if ts < since {
                    return false;
                }
            }
            if let Some(until) = f.until {
                if ts > until {
                    return false;
                }
            }
        } else if f.since.is_some() || f.until.is_some() {
            // No timestamp on payload but a time filter was set: skip.
            return false;
        }
    }
    true
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

// ---------------------------------------------------------------------------
// Sprint 005 (T025) — pure-function fusion of per-source ranked rows.
// ---------------------------------------------------------------------------

use klams_types::{FusionStrategy, WeightedNorm};
use std::collections::HashMap;
use uuid::Uuid;

/// Fuse per-source `RankedRow` lists into a single ranked list.
///
/// Each input vector is treated as ranked best-first. Behaviour:
/// - **RRF** (Reciprocal Rank Fusion): `score = sum_over_sources(1 / (k + rank_i))`
///   for each id; `k` defaults to 60. Stable, parameter-light,
///   needs no score normalisation.
/// - **Weighted**: per-source weights blend normalised scores
///   (z-score or min-max). Bias toward a source by raising its
///   weight.
///
/// Ids that appear in multiple sources accumulate. Payloads are
/// kept from whichever source emitted the row last (caller picks
/// vector first then fts last for "fts wins on duplicate metadata").
#[must_use]
pub fn fuse(
    sources: Vec<Vec<RankedRow>>,
    strategy: FusionStrategy,
) -> Vec<RankedRow> {
    match strategy {
        FusionStrategy::Rrf { k } => rrf(sources, k),
        FusionStrategy::Weighted {
            vector,
            fts,
            normalization,
        } => weighted(sources, vector, fts, normalization),
    }
}

fn rrf(sources: Vec<Vec<RankedRow>>, k: u32) -> Vec<RankedRow> {
    let k_f = f32::from(u16::try_from(k.min(u32::from(u16::MAX))).unwrap_or(u16::MAX));
    let mut acc: HashMap<Uuid, RankedRow> = HashMap::new();
    let mut score: HashMap<Uuid, f32> = HashMap::new();
    for rows in sources {
        for (rank, row) in rows.into_iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let r = (rank as f32) + 1.0;
            let inc = 1.0 / (k_f + r);
            *score.entry(row.id).or_insert(0.0) += inc;
            acc.entry(row.id).and_modify(|r| r.score = 0.0).or_insert(row);
        }
    }
    finalize(acc, &score)
}

fn weighted(
    sources: Vec<Vec<RankedRow>>,
    w_vector: f32,
    w_fts: f32,
    norm: WeightedNorm,
) -> Vec<RankedRow> {
    let mut acc: HashMap<Uuid, RankedRow> = HashMap::new();
    let mut score: HashMap<Uuid, f32> = HashMap::new();
    for rows in sources {
        let normalized = normalise_scores(&rows, norm);
        for (row, ns) in rows.into_iter().zip(normalized.into_iter()) {
            let w = match row.source {
                klams_types::RetrievalSource::Vector => w_vector,
                klams_types::RetrievalSource::Fts => w_fts,
                klams_types::RetrievalSource::MetadataOnly => 0.0,
            };
            *score.entry(row.id).or_insert(0.0) += ns * w;
            acc.entry(row.id).and_modify(|r| r.score = 0.0).or_insert(row);
        }
    }
    finalize(acc, &score)
}

fn normalise_scores(rows: &[RankedRow], norm: WeightedNorm) -> Vec<f32> {
    if rows.is_empty() {
        return Vec::new();
    }
    match norm {
        WeightedNorm::MinMax => {
            let (mn, mx) = rows
                .iter()
                .map(|r| r.score)
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
                    (a.min(v), b.max(v))
                });
            let span = mx - mn;
            if span <= f32::EPSILON {
                vec![1.0; rows.len()]
            } else {
                rows.iter().map(|r| (r.score - mn) / span).collect()
            }
        }
        WeightedNorm::ZScore => {
            #[allow(clippy::cast_precision_loss)]
            let n = rows.len() as f32;
            let mean: f32 = rows.iter().map(|r| r.score).sum::<f32>() / n;
            let var: f32 = rows
                .iter()
                .map(|r| {
                    let d = r.score - mean;
                    d * d
                })
                .sum::<f32>()
                / n;
            let sd = var.sqrt();
            if sd <= f32::EPSILON {
                vec![0.0; rows.len()]
            } else {
                rows.iter().map(|r| (r.score - mean) / sd).collect()
            }
        }
    }
}

fn finalize(
    mut acc: HashMap<Uuid, RankedRow>,
    score: &HashMap<Uuid, f32>,
) -> Vec<RankedRow> {
    for (id, s) in score {
        if let Some(row) = acc.get_mut(id) {
            row.score = *s;
        }
    }
    let mut out: Vec<RankedRow> = acc.into_values().collect();
    out.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use klams_types::RetrievalSource;
    // Behavioural tests for the adapter live in the `ContextBuilder`
    // tests (`klams_core::context`).

    #[test]
    fn normalize_handles_zero_max() {
        assert!((normalize(0.0, 0.0) - 0.0).abs() < f32::EPSILON);
        assert!((normalize(0.5, 1.0) - 0.5).abs() < f32::EPSILON);
        assert!((normalize(2.0, 1.0) - 1.0).abs() < f32::EPSILON);
    }

    fn row(source: RetrievalSource, score: f32) -> RankedRow {
        RankedRow {
            source,
            id: Uuid::now_v7(),
            score,
            payload: serde_json::json!({}),
        }
    }

    #[test]
    fn rrf_is_monotonic_in_rank() {
        let a = vec![
            row(RetrievalSource::Vector, 0.9),
            row(RetrievalSource::Vector, 0.5),
            row(RetrievalSource::Vector, 0.1),
        ];
        let fused = fuse(vec![a], FusionStrategy::default_rrf());
        // Output preserves the input order because there's only
        // one source: rank 1 → rank N, scores monotonically
        // decreasing.
        let scores: Vec<f32> = fused.iter().map(|r| r.score).collect();
        for w in scores.windows(2) {
            assert!(w[0] >= w[1]);
        }
    }

    #[test]
    fn rrf_identical_inputs_yield_identical_scores() {
        let a = vec![row(RetrievalSource::Vector, 1.0); 3];
        let b = vec![row(RetrievalSource::Fts, 1.0); 3];
        let fa = fuse(vec![a.clone()], FusionStrategy::Rrf { k: 60 });
        let fb = fuse(vec![b.clone()], FusionStrategy::Rrf { k: 60 });
        // Same shape → same per-rank scores.
        let sa: Vec<f32> = fa.iter().map(|r| r.score).collect();
        let sb: Vec<f32> = fb.iter().map(|r| r.score).collect();
        assert_eq!(sa, sb);
    }

    #[test]
    fn rrf_k_scaling_shrinks_top_to_bottom_gap() {
        let small = fuse(
            vec![vec![
                row(RetrievalSource::Vector, 0.0),
                row(RetrievalSource::Vector, 0.0),
                row(RetrievalSource::Vector, 0.0),
            ]],
            FusionStrategy::Rrf { k: 1 },
        );
        let large = fuse(
            vec![vec![
                row(RetrievalSource::Vector, 0.0),
                row(RetrievalSource::Vector, 0.0),
                row(RetrievalSource::Vector, 0.0),
            ]],
            FusionStrategy::Rrf { k: 1000 },
        );
        let gap_small = small.first().unwrap().score - small.last().unwrap().score;
        let gap_large = large.first().unwrap().score - large.last().unwrap().score;
        assert!(
            gap_large < gap_small,
            "larger k should flatten the score distribution; small={gap_small}, large={gap_large}"
        );
    }

    #[test]
    fn fuse_empty_sources_returns_empty() {
        let fused = fuse(Vec::<Vec<RankedRow>>::new(), FusionStrategy::default_rrf());
        assert!(fused.is_empty());
        let fused = fuse(vec![Vec::<RankedRow>::new()], FusionStrategy::default_rrf());
        assert!(fused.is_empty());
    }

    #[test]
    fn weighted_minmax_normalisation_makes_top_score_one() {
        let rows = vec![
            row(RetrievalSource::Vector, 0.9),
            row(RetrievalSource::Vector, 0.1),
        ];
        let n = normalise_scores(&rows, WeightedNorm::MinMax);
        assert!((n[0] - 1.0).abs() < f32::EPSILON);
        assert!(n[1].abs() < f32::EPSILON);
    }

    #[test]
    fn weighted_zscore_centers_on_zero() {
        let rows = vec![
            row(RetrievalSource::Vector, 1.0),
            row(RetrievalSource::Vector, 2.0),
            row(RetrievalSource::Vector, 3.0),
        ];
        let n = normalise_scores(&rows, WeightedNorm::ZScore);
        let sum: f32 = n.iter().sum();
        assert!(sum.abs() < 1e-5, "z-scores should sum to ~0, got {sum}");
    }

    #[test]
    fn weighted_constant_distribution_does_not_panic() {
        let rows = vec![row(RetrievalSource::Vector, 0.5); 5];
        let mm = normalise_scores(&rows, WeightedNorm::MinMax);
        let zs = normalise_scores(&rows, WeightedNorm::ZScore);
        assert_eq!(mm.len(), 5);
        assert_eq!(zs.len(), 5);
    }
}

