//! Context bundle assembly.
//!
//! Sprint 005 (Phase 4) — T018. The `ContextBuilder` takes a
//! `ContextRequest`, retrieves rows from the configured
//! [`HybridStore`] sources, applies dedupe + budget rules from
//! FR-003 / FR-004, and produces a `ContextBundle`.
//!
//! For US1 the builder uses two sources only (Vector → knowledge,
//! Fts → facts/events). US2 will plug in real RRF fusion across all
//! three sources without changing this builder's surface.

use crate::tokens::TokenCounter;
use klams_store::{HybridStore, RankedRow, StoreError};
use klams_types::{
    ContextBundle, ContextItem, ContextRequest, ItemKind, RetrievalFilters, RetrievalSource,
    SectionMeta, SectionSource, SectionStatus,
};
use std::collections::{BTreeMap, HashSet};
use tracing::warn;

/// Per-section soft minimum floor expressed as a fraction of the
/// request's `token_budget`. FR-003 requires a "minimum floor per
/// section before greedy fill, not pure global greedy". With three
/// sections and a 10% floor each, 70% of the budget is left for the
/// greedy pass.
const SECTION_FLOOR_FRACTION: f32 = 0.10;

/// Documented top-K to ask each source for before dedupe/budget. The
/// concrete value comes from `[retrieval] per_source_top_k`; this
/// constant is the test default.
pub const DEFAULT_PER_SOURCE_TOP_K: u32 = 100;

/// Build context bundles. Cheap to clone (holds `Arc`s only).
#[derive(Clone)]
pub struct ContextBuilder {
    counter: TokenCounter,
    per_source_top_k: u32,
}

impl std::fmt::Debug for ContextBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextBuilder")
            .field("counter", &self.counter)
            .field("per_source_top_k", &self.per_source_top_k)
            .finish()
    }
}

impl ContextBuilder {
    #[must_use]
    pub fn new(counter: TokenCounter, per_source_top_k: u32) -> Self {
        Self {
            counter,
            per_source_top_k: per_source_top_k.max(1),
        }
    }

    /// Build a bundle. Per-section unavailability is reported in
    /// `sections[*].status` rather than failing the whole call
    /// (FR-011). The only error condition is "all sources
    /// unavailable", which the handler maps to a 503.
    pub async fn build<H: HybridStore + ?Sized>(
        &self,
        hybrid: &H,
        req: &ContextRequest,
    ) -> Result<ContextBundle, BuildError> {
        let filters = req.filters.clone().unwrap_or_default();
        let mut sections: BTreeMap<String, SectionMeta> = BTreeMap::new();

        // ---- retrieval ----------------------------------------------------
        let (vector_rows, vector_status) =
            self.retrieve_one(hybrid, RetrievalSource::Vector, &req.query, &filters)
                .await;
        let (fts_rows, fts_status) =
            self.retrieve_one(hybrid, RetrievalSource::Fts, &req.query, &filters)
                .await;

        // Bucket rows into the three response sections by their
        // `payload.section` tag (set by the adapter).
        let (mut knowledge, mut facts, mut events) =
            (Vec::new(), Vec::new(), Vec::new());
        for row in vector_rows {
            knowledge.push(row);
        }
        for row in fts_rows {
            match row.payload.get("section").and_then(|v| v.as_str()) {
                Some("facts") => facts.push(row),
                Some("events") => events.push(row),
                _ => {}
            }
        }

        // ---- dedupe across sections (FR-004) -----------------------------
        // facts > knowledge for structured attributes (repo+file pair)
        // knowledge > events for prose (repo+file pair)
        let fact_keys: HashSet<String> = facts.iter().filter_map(payload_key).collect();
        knowledge.retain(|r| payload_key(r).is_none_or(|k| !fact_keys.contains(&k)));
        let know_keys: HashSet<String> = knowledge.iter().filter_map(payload_key).collect();
        events.retain(|r| payload_key(r).is_none_or(|k| !know_keys.contains(&k)));

        // ---- score → ContextItem (with token costing) --------------------
        let mut facts_items = score_rows_to_items(&self.counter, facts);
        let mut knowledge_items = score_rows_to_items(&self.counter, knowledge);
        let mut events_items = score_rows_to_items(&self.counter, events);

        // ---- budget allocation (FR-003) ----------------------------------
        let budget = req.token_budget;
        let (facts_keep, knowledge_keep, events_keep, total_spent, truncated) = allocate_budget(
            &mut facts_items,
            &mut knowledge_items,
            &mut events_items,
            budget,
        );

        // ---- section metadata --------------------------------------------
        record_section(
            &mut sections,
            "facts",
            &facts_keep,
            vector_status_combine(fts_status.clone()),
            tokens_in(&facts_keep),
        );
        record_section(
            &mut sections,
            "knowledge",
            &knowledge_keep,
            vector_status_combine(vector_status.clone()),
            tokens_in(&knowledge_keep),
        );
        record_section(
            &mut sections,
            "events",
            &events_keep,
            vector_status_combine(fts_status.clone()),
            tokens_in(&events_keep),
        );

        // All sources unavailable → 503.
        if matches!(vector_status, SourceStatus::Unavailable(_))
            && matches!(fts_status, SourceStatus::Unavailable(_))
        {
            return Err(BuildError::AllSourcesUnavailable);
        }

        Ok(ContextBundle {
            facts: facts_keep,
            knowledge: knowledge_keep,
            events: events_keep,
            total_spent,
            truncated,
            token_encoder: self.counter.encoder_id(),
            sections,
        })
    }

    async fn retrieve_one<H: HybridStore + ?Sized>(
        &self,
        hybrid: &H,
        source: RetrievalSource,
        query: &str,
        filters: &RetrievalFilters,
    ) -> (Vec<RankedRow>, SourceStatus) {
        match hybrid
            .retrieve(source, query, filters, self.per_source_top_k)
            .await
        {
            Ok(rows) => (rows, SourceStatus::Ok),
            Err(err) => {
                let reason = source_unavailable_reason(&err);
                warn!(?source, error = %err, "hybrid source unavailable; section will be degraded");
                (Vec::new(), SourceStatus::Unavailable(reason))
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("all retrieval sources are unavailable")]
    AllSourcesUnavailable,
}

#[derive(Debug, Clone)]
enum SourceStatus {
    Ok,
    Unavailable(String),
}

fn vector_status_combine(s: SourceStatus) -> (SectionStatus, Option<String>) {
    match s {
        SourceStatus::Ok => (SectionStatus::Ok, None),
        SourceStatus::Unavailable(reason) => (SectionStatus::Unavailable, Some(reason)),
    }
}

fn source_unavailable_reason(err: &StoreError) -> String {
    match err {
        StoreError::Backend(m) => format!("backend: {m}"),
        StoreError::Embedding(m) => format!("embedding: {m}"),
        StoreError::Other(m) => format!("other: {m}"),
        other => format!("{other}"),
    }
}

/// Tag used by the dedupe pass. Returns `None` when the row has no
/// `repo` + `file` pair (so we don't dedupe by an empty key).
fn payload_key(row: &RankedRow) -> Option<String> {
    let repo = row.payload.get("repo").and_then(|v| v.as_str())?;
    let file = row.payload.get("file").and_then(|v| v.as_str())?;
    if repo.is_empty() || file.is_empty() {
        return None;
    }
    Some(format!("{repo}::{file}"))
}

fn score_rows_to_items(counter: &TokenCounter, rows: Vec<RankedRow>) -> Vec<ContextItem> {
    rows.into_iter()
        .map(|r| {
            let tokens = counter.count_json(&r.payload);
            ContextItem {
                kind: ItemKind::Raw,
                id: r.id,
                score: r.score,
                tokens,
                payload: r.payload,
                source_ids: None,
            }
        })
        .collect()
}

/// Greedy fill with per-section minimum floors. Returns the items
/// actually kept (sorted by score, highest first), total tokens
/// spent, and a `truncated` flag that is true iff any item from any
/// section was dropped.
///
/// Algorithm:
/// 1. Sort each section by score descending.
/// 2. Each section gets a soft floor (10% of budget by default);
///    take items in score order until the floor is met or the
///    section runs dry.
/// 3. Pool remaining items from all sections, sort by score, greedy
///    fill until the budget is exhausted.
/// 4. Single-item-larger-than-budget edge case: if no item fit
///    anywhere and the highest-scoring item across all sections has
///    `tokens > budget`, keep that one item with `truncated: true`
///    (FR / acceptance scenario 3).
fn allocate_budget(
    facts: &mut Vec<ContextItem>,
    knowledge: &mut Vec<ContextItem>,
    events: &mut Vec<ContextItem>,
    budget: u32,
) -> (Vec<ContextItem>, Vec<ContextItem>, Vec<ContextItem>, u32, bool) {
    let by_score_desc = |a: &ContextItem, b: &ContextItem| {
        b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
    };
    facts.sort_by(by_score_desc);
    knowledge.sort_by(by_score_desc);
    events.sort_by(by_score_desc);

    let total_input = facts.len() + knowledge.len() + events.len();
    if total_input == 0 || budget == 0 {
        let truncated = total_input > 0;
        return (Vec::new(), Vec::new(), Vec::new(), 0, truncated);
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let floor = ((budget as f32) * SECTION_FLOOR_FRACTION) as u32;

    // Capture the single highest-score candidate per section up
    // front so the edge-case branch below can rescue it after the
    // floor / pool passes drain the originals.
    let top_facts = facts.first().cloned();
    let top_know = knowledge.first().cloned();
    let top_events = events.first().cloned();

    let mut keep_facts = Vec::new();
    let mut keep_know = Vec::new();
    let mut keep_events = Vec::new();
    let mut spent: u32 = 0;
    let mut rest_facts: Vec<ContextItem> = Vec::new();
    let mut rest_know: Vec<ContextItem> = Vec::new();
    let mut rest_events: Vec<ContextItem> = Vec::new();

    take_floor(facts, &mut keep_facts, &mut rest_facts, floor, &mut spent, budget);
    take_floor(knowledge, &mut keep_know, &mut rest_know, floor, &mut spent, budget);
    take_floor(events, &mut keep_events, &mut rest_events, floor, &mut spent, budget);

    // Pool remaining and greedy-fill.
    let mut pool: Vec<(usize, ContextItem)> = Vec::new();
    for it in rest_facts {
        pool.push((0, it));
    }
    for it in rest_know {
        pool.push((1, it));
    }
    for it in rest_events {
        pool.push((2, it));
    }
    pool.sort_by(|a, b| {
        b.1.score.partial_cmp(&a.1.score).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut dropped = false;
    for (section_idx, item) in pool {
        if spent.saturating_add(item.tokens) <= budget {
            spent += item.tokens;
            match section_idx {
                0 => keep_facts.push(item),
                1 => keep_know.push(item),
                _ => keep_events.push(item),
            }
        } else {
            dropped = true;
        }
    }

    // Edge: nothing fit and budget < highest-score item's tokens →
    // surface that item with `truncated: true` (acceptance #3).
    let total_kept = keep_facts.len() + keep_know.len() + keep_events.len();
    if total_kept == 0 {
        let cand = [
            (0usize, top_facts),
            (1, top_know),
            (2, top_events),
        ];
        let best = cand
            .into_iter()
            .filter_map(|(i, o)| o.map(|it| (i, it)))
            .max_by(|a, b| {
                a.1.score
                    .partial_cmp(&b.1.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((section_idx, it)) = best {
            spent = it.tokens;
            match section_idx {
                0 => keep_facts.push(it),
                1 => keep_know.push(it),
                _ => keep_events.push(it),
            }
            dropped = true;
        }
    }

    keep_facts.sort_by(by_score_desc);
    keep_know.sort_by(by_score_desc);
    keep_events.sort_by(by_score_desc);

    (keep_facts, keep_know, keep_events, spent, dropped)
}

fn take_floor(
    section: &mut Vec<ContextItem>,
    keep: &mut Vec<ContextItem>,
    rest: &mut Vec<ContextItem>,
    floor: u32,
    spent: &mut u32,
    budget: u32,
) {
    let mut section_spent: u32 = 0;
    while let Some(it) = section.pop_front() {
        let would = spent.saturating_add(it.tokens);
        if section_spent < floor && would <= budget {
            section_spent += it.tokens;
            *spent = would;
            keep.push(it);
        } else {
            rest.push(it);
        }
    }
}

/// `Vec` does not have `pop_front`; tiny helper to enable the
/// floor-fill loop while preserving sort order.
trait PopFront<T> {
    fn pop_front(&mut self) -> Option<T>;
}
impl<T> PopFront<T> for Vec<T> {
    fn pop_front(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            Some(self.remove(0))
        }
    }
}

fn record_section(
    sections: &mut BTreeMap<String, SectionMeta>,
    name: &str,
    items: &[ContextItem],
    status: (SectionStatus, Option<String>),
    tokens_spent: u32,
) {
    let (status, reason) = status;
    sections.insert(
        name.into(),
        SectionMeta {
            count: u32::try_from(items.len()).unwrap_or(u32::MAX),
            tokens_spent,
            source: SectionSource::Raw,
            status,
            degraded_reason: reason,
        },
    );
}

fn tokens_in(items: &[ContextItem]) -> u32 {
    items.iter().map(|i| i.tokens).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn item(score: f32, tokens: u32) -> ContextItem {
        ContextItem {
            kind: ItemKind::Raw,
            id: Uuid::new_v4(),
            score,
            tokens,
            payload: serde_json::json!({}),
            source_ids: None,
        }
    }

    #[test]
    fn budget_zero_yields_empty_with_truncated_flag() {
        let mut f = vec![item(0.9, 100)];
        let mut k: Vec<ContextItem> = Vec::new();
        let mut e: Vec<ContextItem> = Vec::new();
        let (fk, kk, ek, spent, truncated) = allocate_budget(&mut f, &mut k, &mut e, 0);
        assert!(fk.is_empty() && kk.is_empty() && ek.is_empty());
        assert_eq!(spent, 0);
        assert!(truncated);
    }

    #[test]
    fn greedy_fills_inside_budget() {
        let mut f = vec![item(0.9, 50), item(0.5, 50)];
        let mut k = vec![item(0.8, 50)];
        let mut e = vec![item(0.7, 50)];
        let (fk, kk, ek, spent, truncated) = allocate_budget(&mut f, &mut k, &mut e, 200);
        assert_eq!(fk.len() + kk.len() + ek.len(), 4);
        assert_eq!(spent, 200);
        assert!(!truncated);
    }

    #[test]
    fn floors_protect_lower_scoring_sections() {
        // 1000-token budget, 10% floor = 100 per section.
        // Facts dominate by score but should not eat the whole
        // budget — knowledge + events each get at least one item.
        let mut f = vec![
            item(0.99, 200),
            item(0.98, 200),
            item(0.97, 200),
            item(0.96, 200),
            item(0.95, 200),
        ];
        let mut k = vec![item(0.4, 90)];
        let mut e = vec![item(0.3, 90)];
        let (fk, kk, ek, spent, _truncated) = allocate_budget(&mut f, &mut k, &mut e, 1000);
        assert!(spent <= 1000);
        assert!(!fk.is_empty());
        assert_eq!(kk.len(), 1, "knowledge floor should admit its one item");
        assert_eq!(ek.len(), 1, "events floor should admit its one item");
    }

    #[test]
    fn truncated_set_when_items_dropped() {
        let mut f = vec![item(0.9, 100), item(0.5, 100), item(0.4, 100)];
        let mut k: Vec<ContextItem> = Vec::new();
        let mut e: Vec<ContextItem> = Vec::new();
        let (_fk, _kk, _ek, spent, truncated) = allocate_budget(&mut f, &mut k, &mut e, 150);
        assert!(spent <= 150);
        assert!(truncated);
    }

    #[test]
    fn single_item_larger_than_budget_still_returned() {
        let mut f = vec![item(0.9, 1000)];
        let mut k: Vec<ContextItem> = Vec::new();
        let mut e: Vec<ContextItem> = Vec::new();
        let (fk, _kk, _ek, spent, truncated) = allocate_budget(&mut f, &mut k, &mut e, 200);
        assert_eq!(fk.len(), 1);
        assert_eq!(spent, 1000);
        assert!(truncated);
    }

    #[test]
    fn dedupe_key_requires_both_repo_and_file() {
        let r1 = RankedRow {
            source: RetrievalSource::Fts,
            id: Uuid::new_v4(),
            score: 1.0,
            payload: serde_json::json!({"repo": "klams", "file": "src/lib.rs"}),
        };
        let r2 = RankedRow {
            source: RetrievalSource::Fts,
            id: Uuid::new_v4(),
            score: 1.0,
            payload: serde_json::json!({"repo": "klams"}),
        };
        assert_eq!(payload_key(&r1).as_deref(), Some("klams::src/lib.rs"));
        assert!(payload_key(&r2).is_none());
    }
}
