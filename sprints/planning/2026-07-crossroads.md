# klams at the crossroads — review & direction (2026-07)

**Status:** Review complete — decisions pending Ken's edits; the agreed
queue lives in [roadmap.md](roadmap.md).  
**Date:** 2026-07-10  
**Context:** Sprints 001–020 shipped. All korg klams work items are
resolved. The MCP surface is live and verified for Claude Code, GHCP,
kyac, and klams-mind. The original phased plan (001–012) and the WI-259
"keep klams" decision are archived in [archive/](archive/).  

---

## 1. Where klams actually is

**The plumbing era is over.** Twenty sprints built: the three-kind
memory model (facts / events / knowledge) with trust, decay, dissents,
and attribution; REST + MCP surfaces with scoped bearer auth (hot-
reloadable since 018); scanner/monitor ingestion; backups (broken
silently for 40 days, fixed + verified in 020); telemetry that now
tells the truth; and schema-compat work (018/019) that made Anthropic
agents first-class clients.

**What changed this week is the important part:** agents are actually
*using* it. Claude Code and Copilot are wired user-scope on three
machines, kyac writes run reports through the memory seam, and
klams-mind's extraction pipeline distills session transcripts into
proposed facts. For the first time, retrieval quality has customers.

**The honest quality read** (from this week's live use):

- `memory_search` over the ~94k scanner-ingested knowledge chunks
  frequently returns **heading-only fragments** — real examples
  returned to an agent this session: `"## MCP tools"`,
  `"# PHASE 8 — Restore Data"`, `"## R-05: Mode TOML Schema Design"`.
  These are retrieval hits that cost tokens and answer nothing. The
  store and the search pipeline performed fine; **the chunks are the
  problem.**
- Agent-authored memories (the `claude`, `kyac`, `klams-mind` authors'
  writes) are dense and useful — the contrast with scanner chunks is
  stark.
- Exact-identifier queries (error codes, hostnames, function names,
  config keys) are served only by embeddings for knowledge; NL-trained
  embeddings are weak at this (krag's documented lesson: they rank
  docstrings over raw code constants).

**Codebase health** (2026-07-10 review, full findings in §5): sound
bones, almost no dead weight — but the review found two P0s that
reframe this whole document: the scanner **leaks stale chunks on every
file edit** (old points are never deleted; the corpus has been
re-polluting itself since sprint 010's one-time purge), and the MCP
search path — where real traffic flows — **ranks by sorting raw scores
across incomparable scales** (Qdrant cosine vs boosted `ts_rank_cd`),
one of *three* divergent ranking implementations in the tree. Corpus
hygiene and ranking unification are therefore prerequisites, not
nice-to-haves.

## 2. The three questions

### 2.1 Qdrant, OpenSearch, or both?

Framed the right way — *is full-text indexing valuable for agent use?*
— the answer is **yes, and we already half-have it**. Facts and events
get Postgres FTS; the knowledge corpus (the big one) is
**embeddings-only**. Agents ask two shapes of question: fuzzy/semantic
("what did we learn about X") where vectors win, and exact/lexical
("who references `KYAC_LOCAL_BASE_URL`", "error 30 lockfile") where
BM25-style search wins and embeddings are demonstrably weak.

What OpenSearch would buy (per the kris trade study,
`~/src/ai/kris/docs/opensearch-research/`, still sound): native hybrid
query (BM25 + kNN in one request), **built-in score normalization**
(which is exactly the cosine-vs-ts_rank fusion problem 016 documented
and deferred), per-field analyzers for code vs prose, regex/substring
queries, pre-filtered kNN (the D-004 filter-pushdown debt), and
aggregations. An instance is already running on kubs0 (kris leftovers,
idle 13 days — needs a licensing-of-attention decision either way:
adopt it or decommission it).

The counterweight is written in kris's grave: **kris died of
infrastructure scope escalation — two plumbing sprints, including
precisely this Qdrant→OpenSearch swap, before any intelligence
shipped.** klams just escaped its own plumbing era; walking straight
into a store migration would be the same trap.

**Recommendation — sequence it so the decision is empirical:**

1. **Fix chunking first** (§2.2). Perfect retrieval over junk chunks
   is still junk. Any store comparison run against the current corpus
   measures noise.
2. **Build the measurement** (§2.3): an identifier-heavy query suite
   in the klams-mind eval harness + a zero/low-score "miss log" on the
   klams side. This turns "is lexical search valuable" from a taste
   question into a number.
3. **Then trial, don't migrate.** If the evals confirm the lexical
   gap, the cheap experiment is a *third search source* behind the
   existing fusion (the composite store already merges two sources):
   either (a) mirror knowledge chunk text into Postgres FTS —
   smallest possible step, no new infra — or (b) index knowledge into
   the already-running OpenSearch and fuse its BM25 results.
   Qdrant stays the vector store either way. Only if OpenSearch
   proves itself as a lexical source AND its kNN looks like it could
   absorb the vector role do we even discuss consolidation — as its
   own decision, later, never a big-bang swap.

### 2.2 RAG for code, "the purpose of tokenmaster", and capabilities

TokenMaster's purpose, distilled (full analysis archived in
[archive/tokenmaster-integration/](archive/tokenmaster-integration/)):
**pay once to understand a codebase's structure, answer structural
questions from a prebuilt graph instead of re-deriving them by grep,
and — the most transferable lesson — *enforce* the cheap path via the
agent's routing instructions (TMX measured 0/15 tool usage when merely
offered, 8/8 when enforced).** Its self-acknowledged gap is durable
semantic memory, which is klams. The archived F1 vision — klams hosts
the structural graph, the scanner keeps it fresh — remains the right
end-state; the question is on-ramp order.

The staged path:

1. **Scanner v2 (the foundation, and the fix for today's observed
   junk).** Code-aware chunking via tree-sitter (krag salvage: the
   concept is proven in-house), context-preserving markdown chunks
   (heading *path* + body, never a bare heading), chunk metadata
   (language, symbol names, heading breadcrumbs) stored as payload.
   This is prerequisite for every other thread: better chunks improve
   retrieval regardless of store, and symbol extraction is the first
   half of graph edges.
2. **Structural edge layer (graph memory) as a timeboxed spike.**
   Symbols + typed edges (`calls`/`imports`/`inherits`, with
   confidence), TMX-verb MCP tools (`callers`/`callees`/`impact`)
   with token-bounded caps. Embed or shell to graphify rather than
   reinventing AST resolution; klams's trust/decay model already fits
   inferred-vs-resolved edge confidence. The scanner's mtime-cursor
   incremental model makes "never stale" a differentiator TMX can't
   match.
3. **Routing enforcement now, for free.** The
   `docs/klams-mcp-for-agents.md` blurb *offers* klams; TMX proved
   offering is worth ~nothing. Rewrite the blurb as routing rules
   ("recall-shaped question → memory_search FIRST, before grep/web")
   and propagate to the instruction files on all three machines. This
   is a docs change with outsized returns and zero risk.

**The capability index** (the "what can we do" buzz): agents planning
work need "where is X tracked, what models are available, what
services exist" — today that knowledge lives in korg (WIs, reports,
proposals), kvllm (model evals), k-homelab (machine recipes), and
nowhere searchable together. The scanner already walks filesystems;
teaching it (or a sibling feeder) to ingest **structured sources** —
korg via its Postgres/MCP, kvllm eval results, deployed-service
inventory — as knowledge with `kind`/source metadata makes klams the
homelab's capability catalog. This is cheap (the write path exists),
distinctly agent-valuable, and pairs with the routing rules ("planning
question → search klams first"). Staleness is handled the same way as
files: re-scan the source of record; klams is the index, never the
system of record.

### 2.3 Memory pruning, testing, and "are we capturing the right things"

Division of labor is already decided (WI-259): **klams provides
enablement, klams-mind provides intelligence.** The pruning/curation
brains are korg #271 (contradiction detection) and #272
(consolidation) in klams-mind's queue. What klams owes that work:

- **Usefulness signal** (carried forward from the archived backlog —
  the best surviving item): an MCP feedback verb (e.g.
  `memory_feedback` / acknowledge-useful) plus `useful_count` /
  `last_useful_at` on memories, feeding decay as a boost term.
  Opt-in, never forced per-retrieval. This is the ground truth for
  "are memories valuable" — retrieval counts only prove something was
  *touched*.
- **The miss log**: record zero-hit and low-top-score queries (query
  text, caller, scores) — this is the "tune scanners based on agent
  use" loop. Misses tell us what agents *wanted* and didn't get;
  that drives chunking fixes, new scan sources, and capability-index
  priorities. A Grafana zero-hit-rate panel makes it visible.
- **Knowledge lifecycle**: verify + harden scanner staleness handling
  (changed/deleted files must remove or supersede their chunks), and
  decide whether knowledge gets decay (it currently doesn't; facts
  do).
- **The eval gate**: klams-mind's harness (golden questions, 4/4
  baseline) is the seed. Grow it two ways: identifier-heavy queries
  (feeds the §2.1 decision) and questions mined from real session
  transcripts (the extraction pipeline already parses them — "would
  klams have answered what this session actually needed?"). Run it
  on a cadence like a test suite; a regression is a bug.

## 3. What we deliberately are NOT doing

- **No store migration on taste** — see kris.
- **No greenfield anything** — WI-259 stands.
- **Viewport features stay parked** (surviving asks recorded in
  roadmap Later) until agent-facing quality work lands; the viewport
  serves the operator, and the operator's biggest problem right now
  is chunk quality, not columns.
- **ATV-StarterKit migration** — superseded by the 013 lightweight
  workflow; closed permanently.
- **Multi-vector embeddings** — real, but sequenced *after* scanner
  v2 (chunking determines what's worth embedding twice) and likely
  alongside an embedding-model upgrade (the 014 re-embed runbook
  exists for exactly this).

## 4. Archived-doc salvage (what carried forward)

From `archive/backlog.md` → roadmap Later: graph memory (→ §2.2 #2),
multi-vector embeddings, usefulness signal (→ §2.3), viewport
source/trust + decay surfacing, cross-machine caching, multi-agent
scratchpad, cloud backup sync, viewport self-update/signing.
Dropped as no-longer-relevant: memory diff/replay (superseded by
klams-mind extraction), ATV migration. `envfact-schema-analysis.md`,
`viewport.md`, `plan.md`, `current-path.md`, and the WI-259 docs are
historical records — decisions extracted, nothing else pending in
them. `tokenmaster-integration/` remains the reference for the graph
work (§2.2).

## 5. Codebase review findings (2026-07-10)

Full review by a read-only agent pass over the workspace; prioritized.
File refs are the anchors — verify before acting on any of them.

### P0 — corpus & ranking correctness

1. **Edited files leak stale chunks forever.** `scan_root` re-publishes
   a changed file's chunks but never deletes its previous points
   (`crates/klams-scanner/src/lib.rs:73-89`); deletion happens only for
   *vanished* files (`lib.rs:98-113`). Changed chunks create new points
   while old versions stay live and searchable. The e2e test's doc
   comment *claims* "old content is gone" after an edit but never
   asserts it (`crates/klams-service/tests/us3d_scanner_e2e.rs:5-6`).
   Sprint 010 purged 14,260 stale chunks once; edit-churn on `~/src`
   has been re-accumulating them since. Fix is cheap: call the existing
   `POST /memory/knowledge/delete?source_file=` before re-publishing a
   changed file — endpoint + store support already exist.
2. **Three ranking implementations, real traffic uses the worst.**
   MCP `memory_search` sorts raw scores across incomparable scales
   (`crates/klams-mcp/src/tools/memory_search.rs:235-240`; facts get
   `ts_rank_cd * decay * confidence * ln-use boosts`,
   `crates/klams-store/src/postgres.rs:206-209`) — knowledge
   structurally outranks facts/events. REST `/memory/search` does
   per-source max-normalize then round-robin (scores don't drive merged
   order). `/memory/context` does real RRF (`klams-core/src/hybrid.rs`).
   016 documented the mismatch deliberately; with agents live it's due.
3. **Heading-only chunks are structural, not incidental.** Section
   split on every heading with no minimum chunk size
   (`crates/klams-scanner/src/chunk.rs:74-84,118-130`); `is_heading`
   also matches `# ` code comments, fragmenting Python/shell/TOML on
   every top-level comment (`chunk.rs:138-153`).
4. **No file-type filter**: the scanner indexes anything UTF-8 —
   lockfiles, JSON fixtures, SVGs (`walk.rs` skips directories only) —
   a meaningful slice of the ~94k corpus is likely noise.

### P1 — architecture seams that gate the roadmap

5. **klams-mcp bypasses the Store abstraction** — tools reach into
   `state.store.postgres` / `.qdrant` / `.embedder` concretely
   (`crates/klams-mcp/src/tools/mod.rs:88-92`, `memory_search.rs:114+`)
   while klams-api is generic over `trait Store`. Any third retrieval
   source currently needs wiring in ≥3 places; route MCP through the
   adapter seam before adding one.
6. **Chunk structure destroyed twice**: scanner normalize trims every
   line (kills indentation), then the API's `normalize_text` collapses
   newlines to spaces — stored chunks are one long line
   (`chunk.rs:26-47`, `klams-api/src/handlers/knowledge.rs:162-185`).
   Chunk `index` is computed but never transmitted; no heading-path or
   language metadata on the wire — neighbor expansion and
   "prepend section heading" retrieval need a schema addition.
7. **Cross-file dedupe hazard**: the content-hash probe is global, so
   an identical chunk in two files becomes one point owned by the first
   file; deleting that file removes it for both
   (`knowledge.rs:60-77`).
8. Knowledge has **zero lexical search** (facts/events get pg FTS;
   knowledge is ANN-only) — the concrete shape of the §2.1 gap. Options
   costed there; note also Qdrant's own full-text payload index as the
   cheapest (match-based, not BM25-ranked) variant.
9. `[retrieval] fusion` config is parsed + validated but never wired —
   `ContextBuilder::with_fusion` has no production caller
   (`klams-service/src/config.rs:257-293`, `main.rs:169-172`).

### P2 — worth a breather sprint

10. **Upgrades, all mechanical now**: axum 0.7→0.8 (+ axum-prometheus
    lockstep; route syntax `/:id`→`/{id}`), thiserror 1→2, metrics
    0.23→0.24, Qdrant legacy `search_points`→`query_points` (the
    universal API is also the door to server-side hybrid queries),
    Prometheus/Grafana image refresh, pin `rust-toolchain.toml`.
11. Dead code (tiny): deprecated `KlamsClient::healthz()`,
    `clear_cache_for_tests()`, three lint-silencer fns.
12. **Test gaps that matter here**: chunker tests are synthetic-only
    (no golden real-file test — the junk-chunk behaviors are untested,
    arguably enshrined); fusion has no test with realistic
    cosine+ts_rank magnitudes; the 017 rank invariants are DB-gated so
    a fusion regression passes branch CI; TEI batch-embedding path
    unused (needed before any 94k re-index).

### Alignment

The review's independent "top 5" and this document's direction agree:
hygiene → chunking → ranking unification → lexical decision → upgrades.
That ordering is what the [roadmap](roadmap.md) queue now encodes.
