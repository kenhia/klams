# Sprint 016 — Retrieval diagnostics (quality support)

**Branch:** `016-retrieval-diagnostics`
**Type:** feature — make `memory_search` results diagnosable so
klams-mind's retrieval eval harness can explain *why* a query passed
or failed, not just how many hits it got.
**Seed:** [roadmap](../planning/roadmap.md) entry 016; consumer is
klams-mind sprint 002 (eval harness, baseline committed).

## Goal

Today `memory_search` computes a per-hit relevance score internally
then **throws it away** — the tool returns a bare `Vec<PublicMemory>`
(see the pre-016 `Ok(scored.into_iter().map(|(_, m)| m).collect())`).
klams-mind's eval report can therefore only say "5 hits" per query;
when a query eventually fails there is nothing to diagnose with.
Surface the score and the per-source rank on every hit.

## Contract (decided with Ken 2026-07-07 — "always-on envelope")

`memory_search` returns `Vec<ScoredMemory>` (always; no opt-in flag):

```jsonc
[
  { "score": 0.71, "source_rank": 0, "memory": { /* PublicMemory */ } },
  { "score": 0.04, "source_rank": 2, "memory": { /* PublicMemory */ } }
]
```

- `score` — the **raw** per-source relevance score, exposed verbatim
  (see the scale caveat below).
- `source_rank` — 0-based position the hit held **within its own
  source's** result list, before cross-source fusion. Reveals fusion
  reordering (e.g. a knowledge hit that was source_rank 0 but landed
  at global array index 3). Global rank is just the array index, so
  it's not a separate field.
- `memory` — the unchanged `PublicMemory` projection. It already
  carries a `kind` discriminator (fact/knowledge/event), and the
  source backend maps 1:1 to that kind (knowledge → Qdrant ANN,
  fact/event → Postgres FTS), so **no separate `source_kind` field** —
  it would duplicate `memory.kind`. (This trims the field I floated in
  the design question; recorded here as a conscious call.)

New wire type `ScoredMemory` lives in `klams-types` beside `PublicMemory`.

## Scale caveat (surface, don't fix — roadmap item 2 / YAGNI)

The merged sort mixes two incomparable scales: knowledge scores are
Qdrant **cosine similarity** (~0..1), fact/event scores are Postgres
**`ts_rank`** (unbounded, typically ≪1). So a knowledge hit at cosine
0.6 will always outrank a fact at ts_rank 0.05 regardless of true
relevance. This sprint **exposes** that (score + `memory.kind` make it
visible in the eval report) but does **not** normalize or re-rank —
the baseline passes 4/4, so no failing metric demands a fusion fix
yet. Documented as a known limitation; a future sprint acts if the
eval numbers call for it.

## Scope

1. `ScoredMemory { score, source_rank, memory }` in `klams-types`.
2. `memory_search::run` returns `Vec<ScoredMemory>`; assign `source_rank`
   from each source's pre-fusion ordering; keep the existing merged
   sort + `top_k` truncation.
3. Fix klams-side callers (the `mcp_phase4` / `mcp_phase6` integration
   tests read `.id` off the results → `.memory.id`).
4. Update the tool doc string / architecture note; record the scale
   caveat as a known limitation.
5. Cross-project note for klams-mind at
   `~/src/ai/klams-mind/sprints/planning/001-cross-project-note.md`
   so its client (`memory_search() -> list[Memory]`) and eval runner
   (`_to_item` reading `m.text`) update in lockstep.

## Out of scope

- Score normalization / reranking / RRF across sources (the caveat).
- D-004 DB-side filter pushdown.
- REST `/memory/search` and `/memory/context` shape changes — the eval
  harness uses the MCP `memory_search` tool; leave the others until
  something needs them.

## Acceptance

- `just gate-all` green (service + viewport).
- `memory_search` returns `ScoredMemory` envelopes; a unit/integration
  test asserts score is present, `source_rank` reflects per-source
  order, and the merged order still sorts by score desc.
- Scale caveat documented; cross-project note written.

## Chronicle

- (2026-07-07) Opened on merged 015. Confirmed the discard site and
  the cosine-vs-ts_rank scale mismatch while scoping. Ken chose the
  always-on envelope over an opt-in debug flag; dropped the redundant
  `source_kind` field in favour of the existing `memory.kind`.
- (2026-07-07) Implemented. Named the type `ScoredMemory` (not
  `SearchHit`) — `klams_types` already has a `SearchHit` for the *REST*
  `/memory/search` (a flattened preview/payload shape); the MCP tool
  needs to wrap the full `PublicMemory`, so a distinct name avoids the
  collision and the confusion. Verified: `just gate` + `just
  gate-viewport` green; `mcp_phase4::memory_search_smoke` (asserts the
  envelope shape, score present + finite, no `source_kind`, projection
  doesn't leak) and `mcp_phase6` soft-delete/restore search tests pass
  against the live compose stack. Cross-project note written for
  klams-mind.
