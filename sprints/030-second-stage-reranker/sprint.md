# Sprint 030 — Second-stage reranker: cross-encoder over the fused candidate set, eval-gated

**korg:** proposal 686 · WI #685 (M)
**Branch:** `030-second-stage-reranker` · **Version:** 0.1.30
**Run mode:** Auto (Fable 5; same full-auto flow as 028/029).

## Goal

The zero-training half of "ML selects the response top-k" (post-028
brainstorm, design record on #628): a second-stage cross-encoder reranks
the retrieval candidate set before the final page is cut. Ship only if
the 026 eval suite is equal-or-better than the 0.1.29 baseline
(**19/21, 0 regressions**); the 2 known-open cases — both
curated-vs-curated rank-1 inversions that per-hit provenance weighting
cannot see — are the acceptance probes.

## Model verification (#685's "verify live" step) — decided 2026-07-26

The WI's candidate, `Qwen/Qwen3-Reranker-0.6B`, **cannot be served by
TEI today**:

- Probed live on kubs0 (TEI `89-1.9`, CDI GPU): the seq-cls conversion
  (`tomaarsen/Qwen3-Reranker-0.6B-seq-cls`) downloads, then the backend
  refuses — `` `classifier` model type is not supported for Qwen3 ``.
- Upstream confirms: TEI PRs #886/#730/#835 (Qwen3 reranker support)
  are open and unmerged as of 2026-07-26; #698/#795 closed unmerged.
  Issue #643 is the open feature request.

**Fallback: `BAAI/bge-reranker-v2-m3`** — natively supported by TEI
(XLM-RoBERTa classifier arch), 568M params (same VRAM class, ~1.2 GB
fp16), 8k context, strong multilingual reranker. The sprint's
architecture is unchanged (second TEI container, `/rerank`); only the
model id differs from the WI's candidate, and the eval gate — not the
model's pedigree — decides whether the stage ships. Revisit
Qwen3-Reranker when a TEI release merges support (it would be a
one-line compose change).

## Design

- **Placement**: the reranker scores the *knowledge candidate set*
  (global ANN + curated stratum survivors, post dedupe/tag-filter) and
  reorders the within-source rank lists that feed 029's weighted RRF.
  Provenance weights then apply to the RERANKED order (#685's ruling):
  the cross-encoder fixes semantic order within the knowledge source;
  provenance weighting still lifts curated over bulk at fusion. It
  complements 029, not replaces it.
- **Optional and best-effort**: `[retrieval] reranker_url` absent =
  stage off (config-gated rollback); a reranker error logs a warning
  and falls through to the un-reranked order — never fails a search.
- **Window**: `[retrieval] rerank_window` (default 50) caps pairs per
  query. Latency budget ~50–150 ms on GPU; measured below.
- **Truncation**: the reranker container runs `--auto-truncate` ON —
  the opposite of the embedder's standing decision, deliberately: a
  truncated *scoring* signal degrades gracefully, while a rejected
  call would kill the whole stage for one oversized chunk. (The
  embedder's rule exists because silent truncation makes stored
  content unfindable; nothing is stored here.)

## Scope

1. Second compose service `reranker` (TEI, own port 7071, GPU via CDI).
2. `Reranker` client in `klams-store` (TEI `/rerank`, best-effort).
3. Optional rerank stage in `memory_search` between candidate assembly
   and `fuse_in_place`; config plumbed like `[retrieval] fusion`.
4. Eval bake-off with/without; latency measured; ship only if
   equal-or-better with 0 regressions.

Explicitly deferred (unchanged from #685): trained LTR / fine-tuned
reranker, gated behind ~1–2k labeled pairs from the search_sample log.

## Chronicle

- **2026-07-26** — Sprint opened from `korg:686` (marked active).
  Version bumped to 0.1.30. Required reading done: run-notes bootstrap
  section, #685, 029 + 028 sprint docs.
- **2026-07-26** — Model verification (above): Qwen3-Reranker ruled
  out live; bge-reranker-v2-m3 selected and probed on the GPU.
  Measured on the probe container: 50 pairs ≈ 105–112 ms, 10 pairs
  ≈ 25 ms, ~1.4 GB VRAM (2.9 GB total with the embedder). One gotcha
  found by probing: TEI's default `--max-client-batch-size` is 32,
  which 422s a 50-text rerank — the compose service pins 64.
- **2026-07-26 — implementation.** Notes beyond the WI:
  - `TeiReranker` (klams-store): **one attempt, 5 s timeout, no
    retries** — deliberately not the embedder's 3-attempt loop. The
    caller's contract is "skip the stage on error", so retries only
    add latency to a search that can already be served. The client
    refuses short/duplicate/out-of-range index sets (applying one
    would silently drop or duplicate page entries) and breaks score
    ties by submission index (the 029 determinism invariant).
  - The stage reorders the knowledge *within-source list* and
    `curated_order` before `fuse_in_place`, rather than the fused
    output: weighted RRF then applies provenance weights to the
    reranked order, which is how "complements 029" cashes out
    mechanically. Beyond-window candidates trail in prior order.
    `raw_score` stays the cosine (the eval and miss-log thresholds
    read it); rerank latency rides the existing retrieval histogram
    as `op="rerank"`, plus a new `klams_rerank_skipped_total`.
  - Docker-gated `mcp_rerank` suite: a DEAD reranker never fails a
    search (end to end over MCP HTTP); a live rerank permutes but
    never drops/duplicates. `TestServer` gained
    `spawn_isolated_with_reranker`.
  - Found in passing: `phase4_hybrid_retrieval` failing on main —
    the shared `knowledge_items_test` collection had accumulated two
    weeks of seeds; dropped it (it recreates) and the suite is green.
    Not a code change.
- **2026-07-26 — eval bake-off** (029 throwaway pattern: branch-built
  0.1.30 on port 7778 against the live Postgres/Qdrant/TEI + probe
  reranker; own config, creds from compose.env, summarization off):
  - **Reranker OFF: 19/21, 0 regressions** — identical to the 0.1.29
    baseline; the config-gated no-op is real.
  - **Reranker ON: 21/21 (100%), 0 regressions, 2 newly fixed** —
    both known-open curated-vs-curated rank-1 inversions (the
    deferred-MCP-gotcha sibling shadow and the rpidash3 split record)
    close. The cross-encoder alone separates same-tier siblings, as
    #685 hoped; no data consolidation was needed.
  - Live-path stage latency from the service's own histogram over the
    21-query run: **median ~34 ms, p99 ~43 ms** — inside the
    50–150 ms budget.
  - Gate green; full docker-gated suite green locally.

## Deployed 2026-07-26

- Version `0.1.30` live on kubs0 (`/healthz` confirms; release build
  from the squash-merged main, fcd05f5 / PR #32).
- Rollback targets: binaries `0.1.29` via `just rollback` (`.prev` in
  place); the STAGE independently via removing `[retrieval]` from
  `/etc/klams/klams.toml` (backup at `klams.toml.bak-030`) or just
  stopping `klams-reranker` (best-effort stage degrades to the 0.1.29
  order, `klams_rerank_skipped_total` counts it).
- Migrations applied: **none** (0012 remains latest) — binary rollback
  is clean.
- New container: `klams-reranker` (TEI `89-1.9`, bge-reranker-v2-m3,
  port 7071, GPU via CDI; ~2.9 GB VRAM total with the embedder).
- **Official eval baseline against live 0.1.30: 21/21 (100%), 0
  regressions** (was 19/21 on 0.1.29) — captured in klams-mind
  `evals/baselines/homelab-retrieval.md` (ea7cb46), with both
  formerly-known-open queries promoted to `expect = "pass"`. Rerank
  stage median 32 ms in production (`op="rerank"` histogram).
- Verified live, beyond `/healthz`: journal shows "second-stage
  reranker enabled" (url + window); the two acceptance probes rank 0
  through the full MCP path; units settled (`klams-service`,
  `klams-monitor` active, journal clean); scanner picks up 0.1.30 on
  its next timer fire.
- Config changes made on hosts (documented, no tokens touched):
  `/etc/klams/klams.toml` gained `[retrieval] reranker_url`
  (validated with `--validate-config` before restart);
  `/ai/klams/config/compose.env` gained `RERANKER_MODEL_ID`.
- Found in passing, NOT this sprint's regressions: (1)
  `scripts/verify-mvp.sh` SC-001 is bit-rotted — it sends
  `source: "verify-mvp.sh"` where the API has long required the
  `Source` enum, so `just health`/`just verify` fail 422 regardless of
  deploy; breather-sprint candidate. (2) the shared test-stack
  collection `knowledge_items_test` accumulates seeds across weeks and
  eventually breaks `phase4_hybrid_retrieval`'s semantic-overlap
  assertion — dropped once here; a per-run drop (or moving the suite
  to `spawn_isolated`) is the durable fix.
- Follow-up for Ken / breather: drop the old 384-dim
  `knowledge_items` collection once v2 keeps proving out (028
  follow-up, still open); swap `RERANKER_MODEL_ID` to Qwen3-Reranker
  when TEI merges support (PRs #886/#730/#835).
