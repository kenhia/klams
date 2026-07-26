//! Provenance tiers for retrieval weighting (sprint 029, #644).
//!
//! Knowledge points differ enormously in kind and intent: a gotcha
//! written *because* someone decided it was worth durable recall is not
//! the same signal as a chunk swept in by the scanner. Ranking composes
//! a per-hit weight from the tier; the tier is derived at query time
//! from fields the store already records (`source` + the author's
//! `agent_name`) — nothing new is written to the corpus.
//!
//! Three tiers, not two (Ken's post-028 ruling, korg #628 comment
//! #183): the rpidash3 known-open case is curated-vs-curated — a
//! hand-authored machine record outranked by a klams-mind session
//! extract — so a two-way curated/bulk split cannot even see it.
//!
//! Deliberately NOT `Source::trust_rank()`: that axis was designed for
//! fact-write conflict resolution and ranks `Task` (scanner) above
//! `AgentProposal` (agents) — exactly backwards for retrieval
//! (review F-1.5).

use klams_types::Source;

/// Agents whose `AgentProposal` writes are machine-distilled rather
/// than hand-authored. Today that is exactly klams-mind's session
/// extracts (it registers as `agent_name = "klams-mind"`,
/// `klams-mind/src/klams_mind/cli.py`).
const EXTRACTOR_AGENTS: &[&str] = &["klams-mind"];

/// Raw-score floor for hits force-fused from the curated stratum
/// (#644): an irrelevant curated hit must never be pushed into the
/// page just because the stratum is small. Calibrated for
/// Qwen3-Embedding-0.6B, same scale as `LOW_SCORE_THRESHOLD`: junk
/// floor ~0.35, genuine hits ~0.55–0.71 (sprint 028 measurements).
pub const CURATED_STRATUM_RAW_FLOOR: f32 = 0.45;

/// Relative competitiveness gate for the provenance boost (#644,
/// measured 2026-07-26 on the live corpus): a curated hit gets the
/// tier weight — and stratum membership — only when its raw cosine is
/// within this fraction of the query's best raw score (global and
/// curated pooled). Raw scores share one embedding space, so the ratio
/// is meaningful.
///
/// Without this gate, ANY curated hit above the absolute floor beat
/// EVERY bulk hit (stratum rank + 2.0 weight ≈ 4× a bulk rank-0
/// contribution), and topically-adjacent agent memories (raw 0.60)
/// flooded out the genuine answer (raw 0.75) — the "what container
/// image" eval regression. Calibration points: 0.600/0.755 = 0.79 must
/// be excluded; 0.661/0.770 = 0.86 (the #628 gotcha vs a bulk
/// transcript) must be included. 0.82 splits the measured gap.
pub const CURATED_COMPETITIVE_FRAC: f32 = 0.82;

/// The boost threshold for one query: a hit is boost-eligible when its
/// raw score clears both the absolute junk floor and the relative
/// competitiveness gate against the query's best raw score.
#[must_use]
pub fn boost_threshold(top_raw: f32) -> f32 {
    CURATED_STRATUM_RAW_FLOOR.max(CURATED_COMPETITIVE_FRAC * top_raw)
}

/// Provenance class of a knowledge hit, ordered by expected usefulness
/// for "how do I / why is X / what's the gotcha" queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceTier {
    /// Deliberate `memory_add` writes by a person or an interactive
    /// agent (claude, ghcp, …) — curated, written to be recalled.
    HandAuthored,
    /// LLM-distilled records written in bulk by an extraction pipeline
    /// (klams-mind session extracts) — curated-ish, mid-trust.
    MachineExtracted,
    /// Scanner ingest: indiscriminate repo sweeps. The corpus bulk.
    Bulk,
}

impl ProvenanceTier {
    /// Classify from the stored `source`, the author's `agent_name`
    /// (None when the author lookup had no row — an `AgentProposal`
    /// with an unknown author is still an agent write), and whether the
    /// point carries a `machine`.
    ///
    /// The `machine` gate is load-bearing (review F-2.4: curated =
    /// "`source = AgentProposal`, **no machine**"): the live corpus
    /// holds file-derived content — scanned Claude session transcripts
    /// — written through an agent-proposal path with `machine` set.
    /// Measured 2026-07-26: without this gate those chunks flooded the
    /// curated stratum and regressed the eval. A true `memory_add`
    /// write never has a machine.
    pub fn classify(source: Source, author_agent: Option<&str>, has_machine: bool) -> Self {
        if has_machine {
            return Self::Bulk;
        }
        match source {
            Source::AgentProposal => match author_agent {
                Some(agent) if EXTRACTOR_AGENTS.contains(&agent) => Self::MachineExtracted,
                _ => Self::HandAuthored,
            },
            Source::User => Self::HandAuthored,
            Source::Task | Source::Controller => Self::Bulk,
        }
    }

    /// Fusion weight for this tier (`w` in `w/(k+rank+1)`).
    ///
    /// Start-gentle values per #644, tuned against the 026 eval suite:
    /// with RRF k=60, weight 2.0 lets a curated hit at rank 5 outscore
    /// a bulk hit at rank 0 (2/66 > 1/61) — strong but not a filter;
    /// a bulk hit that genuinely dominates (many list appearances,
    /// better ranks) can still win.
    #[must_use]
    pub fn weight(self) -> f32 {
        match self {
            Self::HandAuthored => 2.0,
            Self::MachineExtracted => 1.5,
            Self::Bulk => 1.0,
        }
    }
}

/// Age demotion for memories that *declared themselves* volatile at
/// write time (#638's `volatility` field; review F-1.4). Stable and
/// undeclared memories never decay — silently burying stable truths is
/// the worst failure mode for this store, so the default is no decay.
///
/// Volatile memories keep full weight for a week, then decay with a
/// 30-day half-life, floored at 0.25 so an old volatile memory is
/// demoted, never disappeared.
#[must_use]
pub fn volatility_demotion(volatility: Option<&str>, age_days: f32) -> f32 {
    if volatility != Some("volatile") {
        return 1.0;
    }
    if age_days <= 7.0 {
        return 1.0;
    }
    (0.5_f32).powf((age_days - 7.0) / 30.0).max(0.25)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_writes_are_hand_authored_even_with_unknown_author() {
        assert_eq!(
            ProvenanceTier::classify(Source::AgentProposal, Some("claude"), false),
            ProvenanceTier::HandAuthored
        );
        assert_eq!(
            ProvenanceTier::classify(Source::AgentProposal, None, false),
            ProvenanceTier::HandAuthored
        );
    }

    #[test]
    fn klams_mind_extracts_are_machine_extracted() {
        assert_eq!(
            ProvenanceTier::classify(Source::AgentProposal, Some("klams-mind"), false),
            ProvenanceTier::MachineExtracted
        );
    }

    #[test]
    fn scanner_and_controller_writes_are_bulk() {
        assert_eq!(
            ProvenanceTier::classify(Source::Task, Some("kubs0-scanner"), false),
            ProvenanceTier::Bulk
        );
        assert_eq!(
            ProvenanceTier::classify(Source::Controller, None, false),
            ProvenanceTier::Bulk
        );
    }

    #[test]
    fn machine_bearing_points_are_bulk_regardless_of_source() {
        // The scanned-transcript case: AgentProposal + machine set.
        assert_eq!(
            ProvenanceTier::classify(Source::AgentProposal, Some("claude"), true),
            ProvenanceTier::Bulk
        );
    }

    #[test]
    fn tier_weights_are_ordered_and_bulk_is_neutral() {
        let hand = ProvenanceTier::HandAuthored.weight();
        let machine = ProvenanceTier::MachineExtracted.weight();
        let bulk = ProvenanceTier::Bulk.weight();
        assert!(hand > machine && machine > bulk);
        assert!(
            (bulk - 1.0).abs() < f32::EPSILON,
            "bulk must stay classic RRF"
        );
    }

    #[test]
    fn undeclared_and_stable_memories_never_decay() {
        assert!((volatility_demotion(None, 1000.0) - 1.0).abs() < f32::EPSILON);
        assert!((volatility_demotion(Some("stable"), 1000.0) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn volatile_memories_decay_after_a_week_with_a_floor() {
        assert!((volatility_demotion(Some("volatile"), 3.0) - 1.0).abs() < f32::EPSILON);
        let at_37 = volatility_demotion(Some("volatile"), 37.0);
        assert!(
            (at_37 - 0.5).abs() < 1e-3,
            "one half-life past the grace week"
        );
        assert!(
            (volatility_demotion(Some("volatile"), 10_000.0) - 0.25).abs() < f32::EPSILON,
            "demoted, never disappeared"
        );
    }
}
