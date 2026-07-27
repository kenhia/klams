# klams — roadmap

**Status:** Active — this is the pointer document: the top entry under
"Sprint queue" is the next sprint. Sprints may also arrive from korg
sprint proposals (see `sprints/018-mcp-client-compat/`) rather than
this queue.  
**Date:** 2026-07-09 (014–018 shipped; 019 in flight on
`019-claude-schema-compat` — korg-driven hotfix: boolean-subschema
fix so Claude Code loads the klams tools, WI 309)  
**Related:** [wi259-recommendation.md](wi259-recommendation.md) ·
[wi259-three-project-review.md](wi259-three-project-review.md) ·
[plan.md](plan.md) (original phased plan, now historical) ·
[current-path.md](current-path.md) (2026-06-09 snapshot, partly superseded) ·
[backlog.md](backlog.md) · companion project: `~/src/ai/klams-mind`

## Where we are

Sprints 001–012 delivered every phase (0–6) of the [original plan](plan.md):
MVP memory, safety/dissents, non-agentic writes, advanced retrieval +
`/memory/context`, backups/ops, the MCP server, observability, attribution,
and operationalized ingestion. Sprint 013 retired spec-kit for the lighter
workflow in [AGENTS.md](../../AGENTS.md).

Standing decisions (from the WI #259 review, 2026-07-05):

- **klams stays.** No greenfield rewrite. klams is the homelab memory store;
  it does not need to "speak" vLLM/LangChain — it needs to stay a good,
  stable MCP/REST surface for agents that do.
- **LLM-smart memory features go to `klams-mind`**, a Python/LangChain
  companion (separate repo) that consumes klams over MCP/REST. klams-side
  work for it is API enablement, not intelligence.
- **ATV-StarterKit migration** (debated in [current-path.md](current-path.md)
  §3) is **superseded** — sprint 013 chose a homegrown lightweight workflow
  instead.
- krag and kris are retired; salvage notes live in
  [wi259-three-project-review.md](wi259-three-project-review.md) §4.

Open bugs: none tracked here. kwi #31 fixed in sprint 015; #32 verified
fixed in production during 015 (knowledge counts populate — close it);
#33 found already fixed during 014 (close it).

## Sprint queue

_The queue is empty — 016 is in flight (`016-retrieval-diagnostics`),
delivering its item 1. Pull the next entry from Later/unscheduled below
(likely the graph-memory / TokenMaster spike, or a fusion-scoring fix if
016's exposed scores show the eval numbers demand it — see the score
scale caveat in `sprints/016-retrieval-diagnostics/sprint.md`)._

### 016 — Retrieval quality support (in flight)

klams-mind sprint 002 ported krag's eval-harness design and committed a
baseline. Sprint 016 delivers **item 1** — surface per-hit `score` +
`source_rank` on `memory_search` (`ScoredMemory` envelope) so eval
failures are diagnosable. Item 2 (reranking / D-004 pushdown /
cross-kind score normalization) is **deferred, not built**: the baseline
passes 4/4, so no failing metric demands it yet. 016 *exposed* the
cosine-vs-`ts_rank` scale mismatch as a documented known limitation; a
future sprint acts if the eval numbers call for it (YAGNI).

### Later / unscheduled (from [backlog.md](backlog.md))

Roughly ordered by current appetite; each graduates via a sprint doc:

- **Lightweight graph memory + TokenMaster spike** — the Option B spike
  from [current-path.md](current-path.md) §2 never ran; it needs the now-live
  ingestion data. Outcome informs whether graph memory is pulled forward.
- **Multi-vector embeddings (text + code)** — natural follow-on once 014's
  `Embedder` trait and re-embed machinery exist.
- **Usefulness-signal decay boost** — "this helped" feedback signal;
  pairs well with klams-mind's consolidation work.
- **Viewport surfacing** — source / trust-rank / decay-weight columns
  (deferred from sprint 009 planning until attribution data was solid —
  it now is).
- Viewport self-update, code signing; cloud backup sync; memory
  diff/replay; cross-machine caching; multi-agent coordination memory.

## How to start the next sprint

Per [AGENTS.md](../../AGENTS.md): take the top queue entry, create branch +
`sprints/###-<short-stub>/` (next number), write `sprint.md` (goal, scope,
acceptance — the queue entry above is the seed), build test-first, keep the
chronicle current, ship behind `just gate`. Move the entry out of this queue
when its sprint doc exists.
