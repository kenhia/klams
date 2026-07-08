# Sprint 017 — Search-rank test assertions (016 follow-up)

**Branch:** `017-search-rank-assertions`
**Type:** test-hardening — close a coverage gap left by sprint 016.

## Goal

Sprint 016's acceptance criteria promised a test asserting that
`memory_search` results have a present `score`, that **`source_rank`
reflects per-source order**, and that **the merged order sorts by
score descending**. The shipped `mcp_phase4::memory_search_smoke` only
verified the *shape* (fields present, score finite, projection doesn't
leak, kind/tag filters) — it never asserted either of the two
behavioral guarantees. The implementation is correct by inspection
(`enumerate()` yields the per-source rank; the fusion `sort_by` is
score-descending), but a future fusion-scoring change (roadmap item 2)
could regress either invariant with nothing to catch it.

This sprint adds the missing assertions and fixes stale `SearchHit`
comments/messages left over from the pre-rename `ScoredMemory` type.
Test-only; no production code changes.

## Scope

1. Seed a second knowledge item in `memory_search_smoke` so the
   single-source (knowledge-only) query returns ≥2 hits with real,
   distinct per-source ranks.
2. Assert the unfiltered merged result list is sorted by `score`
   descending (the cross-source fusion invariant).
3. Assert the single-source `source_rank`s are a 0-based contiguous
   ranking and that a lower `source_rank` carries a higher-or-equal
   score (i.e. `source_rank` tracks that source's own ordering).
4. Rename the stale `SearchHit` references in comments/assert messages
   to `ScoredMemory`.

## Out of scope

- Any production behavior change. The invariants already hold; this
  only pins them.
- The deferred fusion-scoring / normalization work (roadmap item 2).

## Acceptance

- `just gate` green (test-only change; viewport untouched).
- `memory_search_smoke` fails if the merged order is not score-desc or
  if `source_rank` stops reflecting per-source order.

## Chronicle

- (2026-07-07) Opened as a follow-up after a post-ship review of 016
  found the two behavioral assertions its acceptance criteria named
  were never written — only the envelope shape was tested. Kept as a
  distinct sprint so the `sprints/` record reflects reality rather than
  amending a merged sprint.
