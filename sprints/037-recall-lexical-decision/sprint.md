# Sprint 037 — Recall quality: the lexical decision

**Proposal:** korg:771 · **Covers:** #333 (L), #729 (S), #632 (L→decision) ·
**Branch:** `037-recall-lexical-decision`

The oldest open P1 (#333, "decide lexical knowledge search once 021–023
data exists") finally has its gate satisfied: the miss log exists, the
eval suite carries an identifier-heavy section built for exactly this
decision, and #729 supplies a live organic failure. This is an
**empirical decision sprint** — every outcome is legitimate, including
"the gap isn't real, decommission OpenSearch". The drift risk to guard
against is building the fun L option without the data demanding it.

## Goal

1. **#729 (S, research) — the entry point.** The standing hygiene search
   (`memory_search` for `klams gotcha`) surfaces zero curated gotchas:
   every curated candidate's raw cosine sits at 0.405–0.450, under the
   0.45 curated-stratum floor, while generic bulk chunks top out at
   0.495. Weigh the three directions in the WI — symptom-phrasing
   practice fix, tag-aware boost/filter convention, lexical signal — and
   land whichever the evidence supports. Gate: the suite's `known_open`
   case (`"klams gotcha"` → `019f95dc-df08` in top-5) promotes to `pass`.
2. **#333 (L, decision + build) — the lexical question.** Knowledge is
   ANN-only. Run the identifier-heavy eval set (suite §7 — recorded as
   measurements, not a bar; which way they fall IS the decision input),
   read the miss log, and fold in #729's live datapoint. Then either
   land the cheapest adequate lexical source behind the now-unified
   fusion (candidate order: Qdrant full-text payload index → Postgres
   FTS mirror of chunk text → BM25 on the idle OpenSearch instance), or
   conclude the gap isn't real and decommission the idle OpenSearch
   container. Trial, not swap — Qdrant stays the vector store. Decide
   before building; the OpenSearch route would grow the sprint.
3. **#632 (research → decision) — the long-payload close-out.** 027
   shipped the honest `PAYLOAD_TOO_LARGE` error, 028 raised the ceiling
   to Qwen3's 32,768 tokens, and the `oversize_write` log (#656,
   migration 0012) has been collecting since — and was empty as of the
   030 breather check. Read the log; if oversize writes remain rare,
   close the chunking half as YAGNI and document the real ceiling in the
   tool description; only if the data argues does server-side chunking
   become its own future proposal.

## Evidence base going in

- Suite §7's own finding (recorded 2026-07-26): the gap is not
  "identifiers never work" — it is "identifiers work only when they are
  already well represented in prose" (`find_knowledge_by_content_hash`
  retrieves; `LOW_SCORE_THRESHOLD` historically vanished, though 028's
  Qwen3 swap promoted it — verify current state at baseline).
- #729's live measurement (2026-07-28): the vague-query failure is
  honest embedding behavior, not a ranking bug — "gotcha" as a literal
  term in the text is what BM25 catches and ANN misses.
- 036 unified the pipeline into `klams-core::retrieval`, so a lexical
  source (if any) lands once, behind the shared weighted-RRF fusion.

## Acceptance

- Each of the three questions closes with a recorded decision backed by
  live measurements, in the WI and this doc — no "keep watching"
  leftovers.
- The `known_open` "klams gotcha" case either promotes to `pass` (fix
  landed) or its WI records why the practice-fix direction was chosen
  instead, with the suite updated to match.
- If a lexical source lands: gated on `just eval` before/after via the
  throwaway-service pattern (029/030 precedent); baseline 21/21 with
  0 regressions holds; ranking changes are deliberate and recorded.
- If the gap is ruled not real: the idle OpenSearch container is
  decommissioned and #333 closes with the evidence.
- #632: `oversize_write` log read and quoted; chunking decision
  recorded; ceiling documented in the `memory_add` tool description if
  not already adequate.
- Gate green; docs updated in-sprint (architecture.md if the pipeline
  gains a source; usage.md for any operator-visible change).

## Chronicle

### Baseline (2026-07-30, live 0.1.36)

`just eval`: **26/27, 0 regressions** — the only failure is the
`known_open` "klams gotcha" case, exactly as documented. Notably
`substring` checks are **11/11**: the entire identifier-heavy section
(§7) passes on pure vector post-028. The synthetic identifier gap the
lexical decision was originally gated on has closed with the Qwen3
swap; the remaining live gap is the vague-query case, which is
different in kind ("gotcha" is a literal term *present in the curated
text* that embeddings won't bind for a two-word query).

### #632 evidence (decided first — cheapest)

- `oversize_write` holds exactly **1 row**, dated 2026-07-26 06:00:
  sprint 027's own deliberate post-deploy verification marker
  (`estimated_tokens=559` vs the then-512 limit). **Zero organic
  oversize writes since instrumentation began**, including the entire
  post-028 (32k-ceiling) era. Server-side chunking closes as YAGNI —
  by the exact instrument korg:651 set up to decide it.
- Tool-description check: `memory_add.text` documents the ceiling
  model-agnostically ("bounded by the embedding model's input length")
  with concrete numbers in the `PAYLOAD_TOO_LARGE` error itself. That
  survives model swaps; no doc change needed. The rpidash3 re-merge
  regression case lives in the eval suite and passes.

### #729 / #333 measurements (2026-07-30, live corpus)

Probes against live Qdrant with the exact pipeline query embedding
(instruct prefix + `klams gotcha`); flagship = `019f95dc-df08` (the
curated MCP-deferred-tools gotcha):

| Probe | Result |
|---|---|
| Unfiltered ANN (pipeline view) | top raw 0.4949 (bulk tokenmaster chunk); flagship absent |
| Unfiltered **exact** search | identical — **no HNSW recall gap** |
| Tags pushdown (`tags=gotcha`, 36 points) | flagship rank 1, raw 0.4053 |
| Text-match subset (points containing "gotcha") | top lowercase match raw 0.3303 — flagship's 0.4053 would rank **#1** of the lexical subset |

Key facts: the high-cosine bulk chunks that win the bare query do
**not** contain the literal word "gotcha"; the curated candidates sit
at 0.405–0.450, under the 0.45 stratum floor (`boost_threshold`), so
the curated stratum contributes nothing. The corpus has 180,591
points, of which 126 carry tags; `gotcha` is the most common tag
(36 points). The MCP `tags` filter is post-ANN `retain` — measured
live, `tags:["gotcha"]` returns **1 hit** instead of a page (it can
only prune the ANN pool, never rescue candidates outside it).
`search_miss` shows no organic misses since 2026-07-26 (the 92-row
spike that day is the eval runner + 027's deliberate probes).

### Decision (#333, and the #729 direction)

**Land the cheapest lexical source: a Qdrant full-text payload index
on `text` + a lexical candidate list in the unified pipeline, fused as
a 5th RRF rank list.** Rationale:

- Identifier queries pass on vector alone (11/11) and the miss log is
  organically empty — nothing justifies a BM25 engine. OpenSearch
  wasn't even idling: both containers had been *exited* for weeks.
- The one live failure is precisely the shape a token-match list
  fixes: the flagship tops the lexical subset (0.4053 > 0.3303), and
  rank-0 in any fused list guarantees page presence (1/61 + tail-rank
  contribution beats bare bulk rank-0's 1/61).
- Conservative by construction: `matches_text` is AND-over-tokens, so
  long prose queries (stopwords never co-occur) produce an empty list
  and fusion is unchanged. The list is deliberately **not**
  boost-gated — the all-tokens match is the relevance evidence, and
  the motivating hit sits below any competitive-score gate.
- #729's direction 3 (lexical signal) is chosen over direction 1
  (symptom-phrasing practice fix — dodges what operators actually
  type) and direction 2 (tag convention — the post-ANN `retain`
  starves it to one result today; fixing that means a filter pushdown,
  a separate small WI worth filing, but it still wouldn't fix the bare
  query the eval gates).

**OpenSearch decommissioned** (both containers, the stray ad-hoc one,
and both images, ~5.9 GB; no volumes existed). Recorded as k-homelab
#798 for fold-in. The kris trade study's BM25 option is closed with
data, not vibes.

### Implementation

- `QdrantStore::connect` builds a full-text index on `text` (word
  tokenizer, lowercase, on-disk) — idempotent, background-built over
  existing points, same error-tolerant pattern as the keyword indexes.
- `Store::search_knowledge_lexical(query_text, query_vector, top_k)`
  (default = not-implemented, like `search_knowledge_curated`):
  filtered kNN over live points with `Condition::matches_text("text",
  query)`.
- `klams-core::retrieval`: `knowledge_candidates` fetches the lexical
  list alongside the stratum; a shared `KnowledgePage::append_aux_list`
  helper (extracted from the stratum-append loop) appends
  not-yet-present hits and returns each list's rank order.
  `lexical_order` is pruned by the collapse/tag-filter ghost check,
  reordered by the rerank stage, and fed to `fuse_in_place` as the 5th
  source. `related()` unchanged (no query text).
- Tests: fuse-level (lexical-only hit lands and leads; shared-id sums
  contributions without duplicating), pipeline-level
  (`LexicalGapStore` models the measured failure shape end-to-end
  through `search`), and a real-Qdrant integration test
  (`lexical_search.rs`: AND semantics, case-insensitivity, the
  distractor-tops-ANN sanity check). MCP/REST mock stores return empty
  lexical lists — no contract change on either surface.
- Docs: architecture.md §2.5 gains stage 5 (renumbered through 15),
  README's hybrid-retrieval line, SVG subtitle notes the diagram
  predates 037 (a redraw rides the next diagram pass — the 033 retro
  harness has the render QA tooling).
