# klams — roadmap

**Status:** Active — this is the pointer document: the top entry under
"Sprint queue" is the next sprint.  
**Date:** 2026-07-06 (014 moved out — in flight on `014-serving-pivot`)  
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

Open bugs: kwi #31 (viewport memory-detail routes 404), #32 (author
`counts.writes` excludes knowledge). (#33, `bench-clean` `?wait=true`,
was found already fixed during sprint 014 — close the kwi item.)

## Sprint queue

### 015 — Companion enablement (klams-mind onboarding) (next)

Give klams-mind what it needs to operate as a first-class, attributable
agent. Scope is demand-driven by klams-mind sprints 001–004 — implement
what it actually needs, no speculative surface:

1. Scoped token + `agent_name = klams-mind` grant; registered author.
2. **External dissent proposal**: today dissents only arise from trust-rank
   conflicts on the *same* fact at write time. klams-mind's semantic
   contradiction detection needs a way to file a dissent linking two
   existing memories (or a fact + proposed correction) via MCP/REST.
   Design the contract in the sprint doc before building.
3. Bulk/paged reads for consolidation passes — `GET /v1/memories` may
   already suffice; verify against klams-mind's consolidation design and
   extend (filters, kind/window paging) only if it falls short.
4. Ride-alongs: kwi #31, #32 (both are read-surface bugs klams-mind and the
   viewport share).

Acceptance: klams-mind can search, add, propose a dissent, and page the
corpus using its own identity; its writes appear under its author in the
viewport.

### 016 — Retrieval quality support

klams-mind sprint 002 ports krag's eval-harness design (TOML query suites;
`substring` / `source_cited` / `no_hallucination` checks) and runs it
against `memory_search`. The klams side:

1. Debug/score metadata on search responses (per-source ranks, fusion
   scores) so eval failures are diagnosable — krag's sprint 015 learned
   this the hard way.
2. Whatever the first baseline report flags: candidate items already known
   are D-004 (DB-side filter pushdown instead of ×3 over-fetch) and a
   reranking stage; **do not pre-build these** — let the eval numbers
   pick the work (YAGNI).

Acceptance: a committed baseline eval report exists (in klams-mind), and
each klams change in this sprint moves a named metric.

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
