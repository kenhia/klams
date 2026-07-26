# Sprint 029 — Ranking and lifecycle: provenance-weighted fusion + supersede/update verbs

**korg:** proposal 653 · WIs #644 (M), #638 (L), #628 (umbrella/acceptance)
**Branch:** `029-ranking-lifecycle` · **Version:** 0.1.29
**Run mode:** Auto (Fable 5, confirmed by Ken 2026-07-26).

## Goal

Make hand-written knowledge win and stale knowledge correctable — the two
things the 2026-07-25 review brief cared most about. Sequenced last
deliberately: needs 025's authz model (ownership/manage tier) and 026's eval
suite (measurability), both in place. 028 left the corpus at 179,762 points
on Qwen3-Embedding-0.6B, eval baseline **18/21, 0 regressions**; the 3
remaining known-open are all rank-inversions/split-record and are this
sprint's acceptance probes.

## Design record (binding)

From the post-028 brainstorm with Ken (korg #628 comment #183) — agreed
direction, not speculation:

- **Curated-stratum search fused as a 4th RRF source.** A filtered ANN
  search over agent-authored points (tiny stratum, same query vector,
  microseconds) always surfaces its best curated matches regardless of
  global rank; it enters the existing RRF fusion seam as a 4th rank list —
  exactly #644's weighted-RRF shape. Rejected alternative: bigger global
  top-k + quota mix (a badly-phrased query can miss the curated target in
  ANY global top-k).
- **Provenance is three tiers, not two**: hand-authored (`claude`/`ghcp`),
  machine-extracted (`klams-mind` session-extracts, mid-trust), bulk
  scanner. The rpidash3-specs known-open is curated-vs-curated — a two-way
  split can't even see it.
- **Weights before quotas**: per-stratum weights first (preserves relevance
  ordering); a quota floor only if the eval shows curated still starving.
- **Guard**: raw_score floor (~0.45, the Qwen3 junk line) on the curated
  stratum so an irrelevant curated hit is never forced into the page.
- **Do NOT reuse `Source::trust_rank`** — it ranks scanner (`Task`=2) above
  agents (`AgentProposal`=1); inverted for this purpose (F-1.5).
- Recency strictly as tiebreak; no blanket knowledge decay (scanner
  `created_at` is scan time). Declared volatility (from #638's write
  surface) gets age-based demotion only when declared.
- Reranker second stage is **sprint 030** (korg:686, #685) — out of scope
  here; it hard-depends on this sprint's fusion seam.

## Scope

1. **#644 — weighted RRF** (`w/(k+rank+1)`): per-hit weight composing
   provenance class and declared volatility; deterministic tie-breaking
   (today cross-kind ties break by HashMap iteration order); curated
   stratum as 4th fusion source. Every change lands with eval
   before/after numbers (hard gate).
2. **#638 — lifecycle verbs**: `memory_supersede(old_id, new_text, …)`
   (atomic replace + `superseded_by`/`supersedes` pointers, hidden via the
   existing soft-delete filter mechanics, restorable via admin surface);
   `memory_update(id, text?, tags?)` (author fixes own record, re-embed on
   text change); similar-on-write (memory_add returns
   `similar_existing: [{id, text_head, author, raw_score}]` above ~0.85
   against agent-authored points — non-blocking, informational).
   Authorization rides #633's manage/ownership model. Out of scope
   (WI-259 division of labor): background contradiction detection and
   consolidation — that's klams-mind (#271/#272); klams ships primitives.
3. **#628 closes as the acceptance case**: Query A repro (natural symptom
   phrasing surfaces the hand-written gotcha above scanner chunks) passes;
   no regression on the 18/21 baseline.

## Acceptance

- The 3 known-open eval cases (all rank-inversion/split-record) pass, or
  each failure is understood and recorded; **0 regressions** vs the 028
  baseline (klams-mind `evals/baselines/homelab-retrieval.md`).
- Fusion is deterministic: identical calls return identical order.
- An agent replaces a stale memory with one call passing `(old_id,
  new_text)`; the old record stops surfacing in `memory_search`; the link
  is inspectable; a second agent's near-identical `memory_add` gets the
  similar-existing nudge.
- `just gate` green; docker-gated suite (`cargo test --workspace --
  --ignored`, test stack up) green locally before merge.
- Docs updated in-sprint; the 0.1.28 search-behavior memory
  (`019f9d5a-539d…`) superseded after deploy ("supersede when 029's
  weighting lands" is written into it).

## Chronicle

- **2026-07-26** — Sprint opened from `korg:653` (marked active). Version
  bumped to 0.1.29. Required reading done: #628 comment #183 (design
  record above), 028 + 026 sprint docs, review F-1.1–F-1.5/F-2.1–F-2.5,
  #644/#638 WI bodies.

- **2026-07-26 — #644 landed** (weighted deterministic RRF + curated
  stratum). Notes beyond the WI:

  - **The weight seam is per-hit, not per-source.** `RankedRow` gained a
    `weight` field (`w/(k+rank+1)`; 1.0 neutral) rather than
    `fuse` taking per-list weights — the three-tier split needs per-row
    granularity *within* the global knowledge list, and splitting the
    list per tier would have renumbered ranks (every tier's best would
    grab rank 0, a much stronger and wrong effect).
  - **Tier detection is query-time, zero migration.** `source` +
    author `agent_name` (already fetched for projection) → hand-authored
    (2.0) / machine-extracted (`klams-mind`, 1.5) / bulk (1.0), in
    `klams_core::provenance`. Confirmed klams-mind registers as
    `agent_name = "klams-mind"` before hardcoding it in
    `EXTRACTOR_AGENTS`.
  - **The MCP fusion dropped payloads** (`Value::Null`), so weights ride
    a side map, and the curated stratum is a 4th `by_kind` list into the
    same `fuse` call. Stratum ids that the tag filter or dup collapse
    removed are pruned before fusion — a fused rank for a ghost id would
    push real results down the page.
  - **REST search hardcodes `default_rrf()`** (ignores `[retrieval]`
    config) — pre-existing, config default is identical, left alone:
    fixing it means threading `FusionStrategy` through `ApiState` (12
    construction sites) for zero behavior change today. REST/context DO
    get the per-hit weighting (author-blind two-tier approximation via
    the adapter) but not the curated stratum.
  - Determinism fix pinned by unit tests that fuse identical inputs 50×
    (pre-029: `HashMap::into_values` order leaked through the
    score-only sort).

- **2026-07-26 — #638 landed** (supersede / update / similar-on-write).
  Notes:

  - **One authorization decision, literally.** `memory_delete`'s
    ownership gate was generalized to `authorize_curation(caller, owner,
    verb)` and shared by all three verbs, per F-1.1's "same capability
    as delete". Both new verbs sit at `Write` scope in
    `required_scope`.
  - **Both verbs refuse scanner chunks** (`NOT_AGENT_AUTHORED`, new
    error code, contract doc updated): derived data updates via
    re-scan; hiding a scanner chunk behind a supersede pointer would
    just get re-ingested on the next scan cycle anyway.
  - **Supersede orders new-first** and rolls the replacement back
    (best-effort) if marking the old point fails — Qdrant has no
    transactions, so the error message states exactly which state the
    store is in. Old point gets the *existing* soft-delete pair plus
    `superseded_by`, so every retrieval filter, the admin surface, and
    restore work unchanged; `payload_to_item` reads the pointers back,
    so `memory_admin_list_deleted` shows `superseded_by` with no
    admin-surface changes at all.
  - **Similar-on-write reuses the curated-stratum search** — the same
    filtered ANN call, run with the write's already-computed embedding
    *before* the upsert (so it can never match itself). Threshold 0.85
    (near-duplicate, not merely topical, under Qwen3's 0.55–0.71
    genuine-hit band); best-effort, a failed nudge never fails a valid
    write.
  - `volatility` (`stable`/`volatile`) plumbed end-to-end
    (args → payload → projection); demotion curve: week grace, 30-day
    half-life, 0.25 floor, only for declared-volatile. Nothing declares
    it yet — the write surface and the ranking hook land together.
  - `memory_add`'s return grew `similar_existing` via a flattened
    wrapper (`MemoryAddOutput`), wire-compatible for existing callers.
  - Clippy pushed one real change: `WriteReply::Knowledge` boxed
    (KnowledgeItem grew past the variant-size bar with the three new
    fields).

- **2026-07-26 — eval tuning: three runs, two live-corpus discoveries.**
  Method: throwaway 0.1.29 service on kubs0 against the live
  Postgres/Qdrant/TEI (own config + read token; summarization off), 026
  suite via `just eval-report`. Baseline: 18/21.

  1. **Run 1 (18/21, 1 regression):** the curated stratum was flooded by
     scanned *Claude session transcripts* — file-derived chunks stored
     with `source = AgentProposal` and `machine` set (a pre-028 ingest
     path). Curated per F-2.4 is `AgentProposal` **and no machine**; the
     first cut dropped the machine half. Fixed in the classifier + the
     stratum filter (`is_empty("machine")`).
  2. **Run 2 (18/21, same regression):** with transcripts excluded,
     *genuine* agent memories flooded instead — a boosted curated hit at
     any stratum rank (~2/61 + 2/6x ≈ 0.06 fused) beats every unboosted
     rank-0 hit (~0.016), so topically-adjacent memories with raw 0.60
     displaced the genuine answer (compose.yml, raw 0.75). The absolute
     0.45 floor cannot see this. Fix: **query-relative boost
     threshold** — stratum membership and tier weight require raw ≥
     max(0.45, 0.82 × the query's best raw, global+curated pooled). Raw
     cosines share one embedding space, so the ratio is meaningful.
     Calibration from live measurements: 0.79 (flooder) excluded, 0.86
     (the #628 gotcha vs a bulk transcript) included.
  3. **Run 3: 19/21 (90%), 0 regressions, 1 newly fixed** — the
     korg:635 split-record case passes for the first time (the record's
     parts are agent memories; the stratum surfaces them at raw 0.64
     over a 0.57 global top; no global top-k ever contained them).

  **The two remaining known-open are no longer curated-vs-bulk.** Both
  targets sit at rank 1 behind a *sibling hand-authored memory* on the
  same topic (the deferred-tools gotcha behind 019f9a36-e558; the
  rpidash3 specs record behind the "joined the fleet" note). Same tier,
  same author — per-hit provenance cannot separate them; that is
  sprint 030's reranker (or data consolidation via the new
  `memory_supersede`). Suite tracking strings updated accordingly;
  the split-record query promoted to `pass`.

- **2026-07-26 — tests.** `just gate` green (113 suites). New unit
  coverage: 5 fusion tests (weights + determinism), 6 provenance-tier
  tests, 3 `fuse_in_place` tests pinning the #628 rank-inversion shape.
  New docker-gated suite `mcp_lifecycle_verbs` (7 tests, green against
  the real stack): the WI acceptance flow (one-call replace → hidden
  from search → link inspectable via admin), cross-author authz for
  both verbs, double-supersede refusal, in-place update with stable id
  (both re-embed and keep-vector paths), scanner-chunk refusal, and the
  similar-on-write nudge (twin write nudged, unrelated write not).
  `mcp_auth`'s advertised-surface expectations updated for the two new
  Write-tier tools. MCP server `instructions` now teach
  supersede-over-delete.
