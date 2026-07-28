# Sprint 033 — Post-arc retrospective, de-slop, and the pitch

**Proposal:** korg:695 · **Covers:** #692 (L) → #694 (M) → #693 (M) · **Branch:** `033-retrospective` · **Version:** 0.1.33

## Goal

The 07-25 deep review launched an intense arc — 025 authz, 026 measurement,
027 ingest correctness, 028 embedder + corpus rebuild, 029 provenance
ranking + lifecycle, 030 reranker, breathers A (korg:689) and B (korg:690).
This sprint steps back and reviews the *result*: check the arc's own work
and thinking, DRY out what six fast sprints duplicated, and produce the
visual artifacts the project now deserves. Hard internal order
**#692 → #694 → #693** — draw and pitch the *reviewed* system, not the
pre-review one. If the sprint runs long, #693 slips first (it depends on
both others).

## Scope

### 1. #692 — Retrospective review (research, L)

Not a bug hunt of old code like the 07-25 review — a retrospective of what
the arc itself built. Four dimensions:

1. **DRY / de-slop.** Known suspects (verify, don't assume): REST search
   hardcoding `default_rrf()` with neither curated stratum nor rerank stage
   (has the REST/MCP divergence become a bug?); the provenance adapter's
   author-blind two-tier approximation on REST/context; collection-swap
   leftovers (v1 references, `.bak` configs); organically-grown `mcp_*`
   test helpers; dead code postdating the #335 list (#335 comment #166).
2. **Re-derive the arc's calibrated constants against live data**, each
   with a verdict — keep (with evidence), retune, or file a WI:
   - 0.82 query-relative boost ratio + 0.45 floor (029 run-2, n=2)
   - tier weights 2.0 / 1.5 / 1.0
   - volatility demotion curve (week grace / 30-day half-life / 0.25
     floor — barely exercised; nothing was declared volatile until 029)
   - `rerank_window = 50`
   - `KNOWLEDGE_OVERFETCH = 2` (calibrated pre-028 on a 44%-duplicate
     corpus that no longer exists)
3. **Eval suite honesty.** 21/21 since 030 = the instrument no longer
   discriminates. Mine the search_sample log for real missed/low-score
   queries; add adversarial cases for the new machinery (volatile
   demotion, superseded-under-paraphrase, reranker on fact/event
   queries). Target: a suite where the next ranking change can fail.
4. **Docs truth-up (structural).** Evaluate folding architecture.md's
   accreted delta sections (2n, 2o, 2p, …) into the main body; verify
   setup.md / usage.md against live kubs0 post-breathers. (#648 in
   breather B did the *content* pass; this is structure.)

**Output:** `docs/reviews/2026-07-XX-retrospective.md` (verdict +
F-sections, same shape as the 07-25 deep review); fixes small enough to
land in-sprint land in-sprint; everything else filed as WIs.

### 2. #694 — Architecture & flow SVGs (feature, M)

Committed under `docs/diagrams/` as plain self-contained `.svg` (no
external fonts; legible in light and dark), embedded from
architecture.md. Likely three:

1. **System topology** — kubs0 + kai: three systemd binaries, Docker
   stack (Postgres, Qdrant, TEI embedder, TEI reranker w/ shared GPU via
   CDI), viewport (Windows), klams-mind (kvllm on kai), backup flow to
   /gratch, MCP/REST surfaces over Tailscale.
2. **Read path** — post-030 pipeline: query → embed (instruct prefix) →
   global ANN + curated-stratum ANN + Postgres FTS → dedupe collapse →
   cross-encoder rerank (best-effort) → provenance-weighted RRF → page.
3. **Write/ingest path** — agent writes (memory_add/supersede/update,
   similar-on-write, authz scopes) vs scanner ingest (fence-aware
   chunking, token gate, machines[] dedupe, delete-before-reindex,
   cursors, both hosts).

Accuracy over beauty: every box/arrow corresponds to something real; the
reviewer test is "could an agent navigate the codebase from this
diagram's names."

### 3. #693 — "The klams pitch" (feature, M)

One self-contained HTML infographic at `docs/pitch/klams-pitch.html`,
linked from README. Value prop, three memory kinds, write/read paths,
lifecycle story, auxiliary cast — with **real numbers pulled live at
authoring time and an as-of date stamp**. Fully offline (inline CSS/JS,
no CDN), theme-aware, responsive. Can embed a simplified derivative of
#694's topology diagram.

## Acceptance

- [ ] Retrospective doc committed under `docs/reviews/` with a verdict
      and per-finding F-sections; every calibrated constant has an
      evidenced keep/retune/file-WI verdict.
- [ ] Eval suite grown to where it can fail: new cases from real
      search_sample traffic + adversarial lifecycle/reranker cases;
      suite passes on live at sprint end (a deliberately-broken ranking
      config demonstrably fails it).
- [ ] In-sprint fixes land with tests; larger findings filed as korg WIs.
- [ ] `docs/diagrams/*.svg` committed, embedded in architecture.md,
      rendering correctly on GitHub in light and dark.
- [ ] `docs/pitch/klams-pitch.html` committed, linked from README,
      renders from file:// with real dated numbers.
- [ ] Gate + docker-gated integration suite green; eval regression-free
      against the 0.1.32 baseline unless a retune is deliberate and
      documented.

## Chronicle

### #692 — the retrospective (2026-07-28)

Full findings: [docs/reviews/2026-07-28-retrospective.md](../../docs/reviews/2026-07-28-retrospective.md). The shape of the day:

- **Evidence gathering**: two delegated repo sweeps (REST/MCP divergence
  map; swap-leftovers + dead code), a full 180,553-point Qdrant census,
  search_sample/search_miss analysis, and a probe harness that embeds
  with the production instruct prefix and queries Qdrant directly —
  exposing the pre-gate curated candidate list the pipeline never shows.
- **Constants: all seven keep.** The 0.82 gate re-derived with ~20 live
  decision points (was n=2): includes ≥0.824, excludes ≤0.813. One
  stale *rationale* found (KNOWLEDGE_OVERFETCH's cross-host-pair story;
  the pairs merged at ingest in 028, the residue is the publish race) —
  comment rewritten, value kept. Volatility curve flagged as never
  having fired live (5 volatile points, all inside grace).
- **The misclassified 10**: the curated machine-gate's prey was removed
  by #688, leaving only collateral — 10 genuine May–July gotchas
  down-tiered to Bulk. Measured harm (axum gotcha losing rank 0 to a
  lower-raw scanner chunk), repaired live via one filtered
  `payload/delete` (before-state: `repair-692-before.json` here),
  verified rank 0 after, gate kept as defense-in-depth.
- **Eval suite 21 → 27** (klams-mind `a12167f`, pushed):
  supersede-under-paraphrase on the real 0.1.28→0.1.30 chain, two
  provenance repair bars, two organic-traffic cases, and the standing
  "klams gotcha" hygiene search as known_open (#729). Counterfactual
  run: a reranker-less throwaway 0.1.33 service scores 24/27 with
  exactly the two 030-promoted cases regressing — the instrument
  discriminates again. Surprise finding: search_sample is ~75% the eval
  suite's own runs (#735).
- **In-sprint fixes** (all TDD where behavior changed): REST `filters`
  accepted-and-ignored since 005 (contract tests); `default_collection()`
  → `knowledge_items_v2` (config test; connect-creates-on-absence made
  the old default a silent empty-store); event_search caller
  attribution; dead `limit()` accessors deleted; bench-clean collection
  + false Postgres-table claims; example config reranker enabled to
  match production; stale bge-small-as-deployed comment batch.
- **Docs**: architecture.md folded 1487 → 830 lines — deltas 2a–2p gone,
  one current description, honest REST/MCP divergence table; fold also
  caught old diagrams naming routes that never existed
  (`POST /v1/facts`). setup.md restore-guard wording fixed.
- **Filed**: #729 vague-query recall (S) · #730 retrieval unification
  (L, measured extract-down plan) · #731 reranker observability (M) ·
  #732 test stack still bge-small/384 (M) · #733 test-helper dedupe (M)
  · #734 upsert_fact v1 rung (S) · #735 search_sample eval pollution
  (klams-mind, S).
- **Verification**: `just gate` green; docker-gated integration suite
  123 passed / 0 failed (stack brought up and torn down); eval 26/27 +
  1 known_open against live.
