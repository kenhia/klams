# klams post-arc retrospective — 2026-07-28

**Reviewer:** Claude (Fable 5), full-source session on kubs0, with live store access.  
**Scope:** sprint 033 (#692) — a retrospective of the 2026-07-25 → 07-27 arc itself: 025 authorization, 026 measurement, 027 ingest correctness, 028 embedder + corpus rebuild, 029 provenance ranking + lifecycle, 030 reranker, breathers A (korg:689) and B (korg:690). Not a bug hunt of old code (the [07-25 deep review](2026-07-25-deep-review.md) was that); a check of what the arc built and the thinking it built with.  
**Inputs:** the workspace at `0813303` (0.1.32 deployed), two delegated full-repo sweeps (retrieval-path divergence; swap leftovers + dead code), a full Qdrant corpus census (180,553 points scrolled), the `search_sample`/`search_miss` logs, live MCP probes with pre-gate curated-candidate visibility (a diagnostic the production pipeline doesn't expose), and a throwaway 0.1.33 service for counterfactual eval runs.  
**Status legend:** ✅ confirmed (code path cited or measured live) · ❓ suspected.  

---

## Verdict — three findings above the fold

1. **The arc's thinking holds.** Every calibrated constant survived re-derivation against live data — and most are now *better* evidenced than when they were set. The 0.82 competitiveness gate was calibrated on **two** data points under deadline; re-measured across 14 queries with pre-gate candidate visibility, every correct inclusion sits at ratio ≥ 0.824 and every correct exclusion at ≤ 0.813. The number was right; now it's right with n≈20 (F-2.1). No constant needed retuning. One constant's *rationale* had rotted while the constant stayed correct (F-2.6) — the comment is rewritten, the value keeps.

2. **The instrument problem recurred one level up — and is fixed.** 026 built measurement because klams "had no idea what agents ask it." By 030 the eval read 21/21 and could not fail; the recalibrated miss log has fired **zero** times on organic traffic since 028 (near-silent by design, but nobody said so anywhere); and the search-sample log — the designated source of future eval queries — turned out to be **~75% the eval suite's own runs** (F-3.2, WI #735). The suite now has 27 cases including the arc's own machinery as adversarial cases (supersede-under-paraphrase, the provenance repair bars, mined organic traffic, a known_open), and it demonstrably discriminates again: a reranker-less service scores 24/27, failing exactly the two cases sprint 030 promoted (F-3.1).

3. **The residual debt is seams, not rot.** The one production-facing wrong *value* found anywhere: `default_collection()` still pointed at the Qdrant collection #684 dropped — combined with connect-time create-on-absence, a config omitting one key would silently run against an empty store (F-1.3, fixed with a test). The one wrong *behavior*: REST `/memory/search` accepted `filters` and discarded them, since sprint 005 (F-1.2, fixed, contract-tested). The one wrong *data*: 10 genuine May–July agent gotchas silently down-tiered to Bulk by a curated-gate whose prey #688 already removed (F-1.7, repaired live, held by two new eval bars). Everything larger is mapped and filed, not mysterious: the REST/MCP read-path split has a measured extraction plan (WI #730), and the test infrastructure still shaped like the pre-028 system is inventoried (WI #732, #733).

---

# Part 1 — DRY / de-slop

## F-1.1 The REST/MCP retrieval divergence is now a mapped seam, not a note ✅ confirmed, filed

Post-030, MCP `memory_search` runs an 11-stage pipeline (embed → global ANN ×2 over-fetch → curated-stratum ANN → boost gate → 3-tier provenance weights → FTS → dedupe collapse → rerank → weighted 4-source RRF → miss/sample logs → projection). REST `/memory/search` runs the `StoreHybridAdapter` subset: same boost gate and dedupe, ×3 over-fetch, an **author-blind two-tier** weight (klams-mind extracts get 2.0 instead of 1.5 — `adapter_knowledge_weight` passes `author_agent: None`, which classifies as HandAuthored), no curated stratum, no rerank, and `FusionStrategy::default_rrf()` hardcoded at `handlers/search.rs` — the `[retrieval]` config is ignored. `memory_related` is bare ANN with none of the above. Duplication inventory: ten near-identical code shapes across the two paths (provenance weight ×2, boost-gate scan ×2, dedupe wrapper ×2 with duplicated test suites, over-fetch constants ×2, `fuse` invocation ×3, section-bucketing ×3, kind-filter enums ×3, author N+1 loop ×2, `top_k` policy ×3, `KnowledgeItem` projection ×2).

**Is the divergence now a bug rather than a note? Yes, at the edges** — F-1.2 was one such edge. The full unification is deliberately *not* done in this sprint: it is L-sized and the blockers are precisely measured (move `projection.rs` down from klams-mcp; a neutral error type; 3 fields into `ApiState` across 12 construction sites). Filed as **WI #730** with the extract-down plan; 031's write-path unification is the precedent — this is the read-side completion of the same program.

## F-1.2 REST `filters` were accepted and silently discarded — fixed ✅ (since sprint 005)

`SearchRequest.filters` deserialized into `Option<serde_json::Value>` and then `search()` built `RetrievalFilters::default()` unconditionally. Every REST search since sprint 005 ran unfiltered no matter what the caller sent — the worst kind of API lie (accepted, unvalidated, ignored). **Fixed in-sprint:** parsed into the same `RetrievalFilters` the `/memory/context` handler feeds the shared adapter (`deny_unknown_fields`, so malformed filters now 400 instead of vanishing). Contract tests: include, exclude, malformed.

## F-1.3 `default_collection()` pointed at the dropped collection — fixed ✅ (the drift that got away from #647)

Sprint 032 (#647) fixed the example config and the reattribute tool but missed the serde default: a `klams.toml` omitting `[qdrant] collection` targeted `knowledge_items` — dropped by #684 — and `QdrantStore::connect` **creates on absence**, so the failure mode was an empty, wrongly-named collection and a service that boots green with zero recall. The only config test parsed the shipped example, which sets the key explicitly, so the default was unexercised. **Fixed in-sprint** (test first: parse a config without the key, assert `knowledge_items_v2`); the connect-on-absence behavior is now called out in its doc comment. Same class, also fixed: `just bench-clean`'s default collection (its Qdrant purge 404'd and reported success), its comment claiming a Postgres `knowledge_items` table exists (sprint-009 leftover, twinned in `tools/bench/README.md`), and the example config shipping the reranker commented out while production runs it (a rebuild-from-example would have silently disabled the 030 stage).

## F-1.4 Collection-swap and embedder-swap leftovers: comments, not code ✅ confirmed, fixed

The 028 swap left no `.bak` files, no orphan units, no unread config keys — breather B got the mechanisms. What it left was *prose*: doc comments asserting bge-small/512/384 as the deployed reality (`embed_limit.rs` module docs and `DEFAULT_MAX_INPUT_TOKENS`, `config.rs`, `embeddings.rs::with_limit`), `qdrant.rs::connect` naming the retired collection, `repair.rs` claiming a Postgres knowledge table, and the 028 reset-runbook still reading as executable with a rollback path (`knowledge_items` v1 + CPU TEI) that #684 removed. All current-truth sites fixed in-sprint; the runbook is left as history (its own banner already declares it superseded). The `DEFAULT_MAX_INPUT_TOKENS = 512` **value** stays deliberately: production sets 32768 explicitly, and a conservative fallback fails loudly at the boundary while a generous one would recreate the silent worker-drop 027 killed.

## F-1.5 Dead code introduced by the arc itself ✅ confirmed

The #335 list predates 028–030; the arc added four of its own:

- `TeiReranker::health()` — zero callers; the reranker is the only stateful dependency invisible to `/healthz` (WI **#731**, with the design constraint that best-effort must not flip overall status).
- `TeiEmbedder::limit()` / `OpenAiCompatEmbedder::limit()` — added in 027 "so ingest paths can gate against exactly what the embedder will enforce"; every ingest path reads config instead; never wired. **Deleted in-sprint.**
- `crates/klams-service/tests/mcp_rerank.rs`'s live-rerank test self-skips unless `TEST_RERANKER_URL` is set — which no documented path sets, and the test compose stack has no reranker service. The only test that exercises a live rerank cannot run (folded into #731).
- `Store::upsert_fact` (v1) — after 031's unification it is unreachable in production yet demanded by every one of 10 mock Stores (WI **#734**).

## F-1.6 Test helpers grew organically, as predicted ✅ confirmed, filed

Six byte-identical copies of `mcp_state_from`; a ~78-line private duplicate of the `McpSession` helper that `tests/common` has exported since 025; three copies of `INIT_BODY`; duplicate SSE parsers, seeders, author builders; four `ensure_collection` copies all hardcoding the retired 384-dim; ten hand-rolled mock `Store` impls each carrying `Ok(vec![0.0; 384])`. Inventoried precisely in WI **#733** (mechanical, one pass); the 384s overlap WI **#732** (F-4.2).

## F-1.7 Ten genuine agent gotchas were silently down-tiered — repaired live ✅ measured, fixed, eval-held

029's curated classifier gates on `machine` being absent, added mid-sprint because scanned Claude-transcript chunks (stored as `AgentProposal` + `machine`) flooded the curated stratum and regressed the eval. Breather B's #688 re-sourced those 13 chunks to `Task` and verified **no current write path can produce `AgentProposal` + `machine`** — after which the gate's only live effect was collateral: the 10 remaining points in that shape are genuine hand-authored gotchas (sprint-007 axum/rmcp/VS-Code-MCP writes, LVGL, TMX, kyac, the TLS decisions — #688's own comment identified them and correctly refused to flip their `source`). **The gate outlived its prey.**

Measured harm before repair: `"axum middleware layer nested router auth not applied"` — the gotcha had the best raw score on the page (0.708 vs 0.651) and still sat at **rank 1 behind a scanner chunk**, because it fused as Bulk (weight 1.0, no stratum membership). Repair: cleared the `machine` key on exactly those 10 points via one filtered Qdrant `payload/delete` (full before-state committed at [`sprints/033-retrospective/repair-692-before.json`](../../sprints/033-retrospective/repair-692-before.json); nightly backups current; points untouched since May–July so the snapshot covers them). Verified: 0 points remain in the shape; both probe gotchas now rank 0 with curated treatment; eval 21/21 → unchanged, then extended. The classifier gate itself **stays** — it is now pure defense-in-depth against the shape recurring (e.g. via an old-backup restore), and two new eval bars hold the repair (F-3.1).

# Part 2 — The arc's calibrated constants, re-derived

Method: for each constant, the live corpus/logs plus a probe harness that embeds with the production instruct prefix and runs both ANN queries directly against Qdrant — exposing the **pre-gate curated candidate list** with raw scores and ratios, which the production pipeline never shows. 14 queries: the suite's curated-target set, the organic post-028 traffic, two misclassification probes, two bulk-answer controls.

| Constant | Where | Verdict | Evidence |
|---|---|---|---|
| `CURATED_COMPETITIVE_FRAC = 0.82` | provenance.rs | **keep** | F-2.1 |
| `CURATED_STRATUM_RAW_FLOOR = 0.45` | provenance.rs | **keep** | F-2.2 |
| `LOW_SCORE_THRESHOLD = 0.45` | memory_search.rs | **keep, re-derive later** | F-2.3 |
| tier weights 2.0 / 1.5 / 1.0 | provenance.rs | **keep** | F-2.4 |
| volatility curve (7d / 30d half-life / 0.25 floor) | provenance.rs | **keep — unexercised** | F-2.5 |
| `KNOWLEDGE_OVERFETCH = 2` | memory_search.rs | **keep, rationale rewritten** | F-2.6 |
| `rerank_window = 50` | config → McpState | **keep — non-binding** | F-2.7 |

## F-2.1 The 0.82 gate: right number, formerly wrong sample size ✅ re-derived

029 calibrated on two points (0.79 must exclude, 0.86 must include) and split the gap. Across the 14-query sweep: correct inclusions at ratios **1.000, 0.996, 0.989, 0.983, 0.957, 0.940, 0.911, 0.899, 0.877×2, 0.871, 0.858, 0.843, 0.840, 0.825, 0.824**; correct exclusions at **0.813, 0.795, 0.785, 0.784, 0.782, 0.779** and below. The decision boundary the corpus actually wants lies in (0.813, 0.824) — 0.82 sits inside it. Two caveats now documented: the band is *narrow* (±0.01 matters; treat any future change as eval-gated), and the borderline inclusions (0.824–0.84) are topically-adjacent-but-harmless rather than wrong — the gate's failure direction is gentle.

## F-2.2 The 0.45 stratum floor ✅ confirmed against the model's real junk floor

The Qwen3 junk floor measured at calibration time reproduces exactly: the nonsense probe ("purple elephant sourdough trampoline…") tops out at raw 0.347; genuine hits run 0.55–0.88 (the 028-era "0.55–0.71" range has *widened upward* as curated memories accumulated — top hits at 0.75–0.88 are now common). The floor only becomes the binding constraint on vague queries ("klams gotcha", top_raw 0.495 → threshold 0.45), where it correctly kept a marginally-relevant curated hit (0.4499) out. Keep.

## F-2.3 The 0.45 miss threshold: honest, but the log is near-silent again — this time by design, now say so ✅

91 of the 93 `search_miss` rows are bge-era (threshold 0.80); since the 028 recalibration the only misses are one deliberate nonsense probe and one emptied-filter zero-hit. **Zero organic misses in the entire post-028 window.** That is the designed behavior — 0.45 sits between the junk floor and the genuine-hit range, so only true nonsense trips it — but it means the miss log is a nonsense-detector, not a weak-recall detector: the one genuinely weak organic query ("klams gotcha", 0.495) sailed over it. Verdict: keep — a higher threshold (say 0.50) would have caught that one query but the organic sample (n≈5) is far too small to recalibrate on, and the vague-query problem is tracked concretely as WI #729. Re-derive when #735 gives the sample log an honest organic stream. The constant's doc comment already says "re-derive from the search-sample log"; that instruction now has a prerequisite.

## F-2.4 Tier weights 2.0/1.5/1.0 ✅ keep

No counter-evidence anywhere in the sweep: every case where a curated hit wrongly lost traced to *data* (F-1.7) or to *phrasing* (#729), never to the weight being too weak or too strong; the bulk-answer control queries show no curated flooding (the gate does that work). The reasoning in `provenance.rs` (2.0 lets curated-at-rank-5 beat bulk-at-rank-0 without being a filter) still describes observed behavior. The REST path's collapse of MachineExtracted into 2.0 is #730's business, not a weights problem.

## F-2.5 The volatility curve has never fired in production ✅ measured — keep, with eyes open

Five points in 180,553 declare `volatility: "volatile"`; all were written 07-26/27, inside the one-week grace window — so `volatility_demotion` has returned 1.0 for every real query ever run. The curve's shape (grace week, 30-day half-life, 0.25 floor) is guarded by unit tests only. This is acceptable for now — the design principle (never decay undeclared memories; demote, never disappear) was the load-bearing decision and it is enforced — but the honest statement is: **the curve is a hypothesis that has not yet met data.** The live eval cannot age a memory; if volatile adoption grows, a docker-gated integration test writing a backdated volatile point is the cheap honest guard. Revisit when the volatile census stops fitting in one hand.

## F-2.6 `KNOWLEDGE_OVERFETCH = 2`: right constant, dead rationale ✅ fixed comment

The comment justified ×2 by "the dominant duplicate shape is a cross-host pair" — a shape #642's ingest dedupe eliminated (one point per content hash, `machines[]` list: 96,386 points carry both hosts). Yet the constant still earns its keep: 71 residual duplicate groups (205 points, 0.11% of corpus — consistent with the known check-then-enqueue publish race, review F-4.3) cluster on exactly the popular content queries hit, so **19% of live searches still collapse ≥1 duplicate** (up to 10 on kpidash-family queries). Without over-fetch those pages would come back short. Comment rewritten with the 2026-07-28 measurements; value unchanged.

## F-2.7 `rerank_window = 50` is non-binding at every default ✅ measured

The rerank stage receives at most the knowledge candidates: `top_k×2` global + ≤`top_k` stratum ≈ 30 for the default `top_k=10` — under the window. It binds only for `top_k > ~17`, i.e. near the `MAX_TOP_K=50` ceiling. Live: 24 rerank calls since deploy, mean ~35 ms (matches 030's measurement); the 5 s client timeout and best-effort skip make the tail harmless. Keep — but know it is currently a *ceiling*, not a *tuning knob*; nothing observable changes between 35 and 50.

# Part 3 — Eval suite honesty

## F-3.1 The suite can fail again ✅ done, demonstrated

21/21 since 030 measured nothing. Grown to **27 cases** (klams-mind `a12167f`), the additions chosen so the arc's own machinery is what's under test:

1. **Supersede-under-paraphrase** — on the real 0.1.28→0.1.29→0.1.30 behavior-note chain: the current note must rank 0 for a paraphrase neither note contains verbatim, and the superseded note's distinctive phrasing must not appear at any rank (F-1.1 of the deep review: "a superseded memory surfacing at rank 7 still gets believed").
2. **Two provenance repair bars** (axum, rmcp → rank 0) — hold F-1.7's repair; a regression means machine-stamped agent writes returned or the gate grew new collateral.
3. **Two organic-traffic cases** — first real harvest from `search_sample` (agent-skills deploy consolidation; session-title plumbing), both verified live before adding.
4. **The standing hygiene search** `"klams gotcha"` as `known_open` against WI #729 — the suite now carries a real, current failure again instead of pretending completeness.

Counterfactual check (the acceptance test for "can it fail"): a throwaway 0.1.33 service with `reranker_url` removed scores **24/27 — 2 regressions + the known_open — and the 2 regressions are exactly the two cases sprint 030 promoted** (deferred-tools literal, rpidash3 specs). The instrument discriminates; the gate exits non-zero.

## F-3.2 The sample log is measuring the measurer ✅ measured, filed

532 `search_sample` rows in the window; ~400 are the eval suite's own runs (21 queries × ~19 executions, caller `klams-mind`, indistinguishable from klams-mind's genuine extraction searches). The log built to answer "what do agents actually ask" mostly answers "what does the eval ask," and mining it naively would feed the suite its own queries back. The 033 harvest worked around it (caller + date filtering, live verification); the durable fix is eval-traffic identity on the klams-mind side — WI **#735**. Also noted there: this sprint's calibration probes are themselves in the log now (attributed to klams-mind's token), which is the same lesson recursed.

## F-3.3 What the organic remainder says ✅ analysis

The honest organic stream is small (a handful of `claude`-caller queries, 07-27/28) but already earned its keep: it surfaced the vague-query recall gap (#729 — the standing hygiene search returns zero curated gotchas; root cause is honest embedding behavior on a two-word query, and it is a live organic data point for the #333 lexical decision, complementing the synthetic identifier set), and two solid regression cases. The mining recipe that worked: `caller NOT IN (eval identities)`, bracket by date, verify each candidate live before asserting a rank. Harvest again when #735 lands and volume exists.

# Part 4 — Docs truth-up (structural)

## F-4.1 architecture.md: fourteen delta sections folded ✅ done this sprint

The doc had become §1 components + a sprint-001-era §2 + deltas 2a–2p + §3 — a fresh reader had to apply fourteen patches mentally, and the top-of-file description contradicted the deltas below it (pre-024 ranking described as current; the reranker missing from the component list and topology; `knowledge_items` in the deployment diagram). Restructured in-sprint: one current description, sprint attributions kept inline as history where load-bearing, delta trail delegated to git history and `sprints/`. (See the sprint doc for the fold's section map.)

## F-4.2 setup.md / usage.md ✅ spot-verified against live kubs0

Breather B's #648 did the content pass two days ago, and it held: the checks this review re-ran (systemd units + timer state, compose services incl. reranker, `/healthz` shape, eval recipes, token flow) match the docs. Two residuals fixed: the restore-guard description named a Postgres `knowledge_items` table that has never existed (knowledge is Qdrant-only; the guard probes the two stores separately), and usage.md's sprint-027 section stated the 512-token bge-small ceiling in present tense.

## F-4.3 The drift class, named ✅ analysis

Every docs finding in this retrospective is one class: **a true statement that kept being asserted after the world changed** — the deployed-model claims, the collection default, the bench harness's self-description, the over-fetch rationale, the example config trailing production. The arc's speed made these; none survived contact with a reader who checked. The repo's existing convention (cite the sprint that made a thing true) is the right one — it makes staleness *datable*. What was missing was a re-reader; this document is that pass, and the next one should ride the next arc's breather rather than wait for a retrospective.

---

# In-sprint changes landed with this review

| Change | Kind | Where |
|---|---|---|
| REST `filters` parsed + validated (was silently ignored since 005) | fix + contract tests | klams-api `search.rs`, `contract_search.rs` |
| `default_collection()` → `knowledge_items_v2` | fix + config test | klams-service `config.rs` |
| 10-point provenance repair (cleared legacy `machine`) | live data op | Qdrant; before-state in sprint dir |
| Eval suite 21 → 27 cases; discriminates again (24/27 sans reranker) | eval | klams-mind `a12167f` |
| `event_search` caller attribution (026 fix never reached it) | fix | klams-mcp `event_search.rs` |
| Dead `limit()` accessors deleted | de-slop | klams-store `embeddings.rs` |
| `bench-clean` collection default + false Postgres-table claims | fix | justfile, tools/bench |
| Example config: reranker enabled to match production | drift fix | deploy/config |
| Stale-comment batch (overfetch rationale, bge-small-as-deployed, connect-creates-on-absence, bench mislabel) | docs-in-code | 6 files |
| architecture.md delta fold + setup.md restore-guard wording | docs | docs/ |

# Work items filed

**#729** vague-query recall ("klams gotcha", research S) · **#730** REST/MCP retrieval unification (task L, extract-down plan) · **#731** reranker observability — healthz blind spot + unrunnable live test + harness gap (chore M) · **#732** test stack still bge-small/384 (chore M) · **#733** test-helper dedupe (chore M) · **#734** `upsert_fact` v1 rung (chore S) · **#735** search_sample eval-traffic identity (klams-mind, chore S).

# Store writes made during this review

The 10-point payload repair (F-1.7, recorded in the sprint dir). Probe traffic: the calibration sweeps ran through the live MCP surface under klams-mind's token, so they appear in `search_sample`/`search_miss` dated 2026-07-28 — flagged in #735 so future miners bracket them. No knowledge memories were written mid-review; the sprint's durable learnings get written back at ship time per standing practice.
