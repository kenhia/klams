# WI #259 — Recommendation: keep klams, pivot the edges

**Date:** 2026-07-05
**Evidence:** [wi259-three-project-review.md](wi259-three-project-review.md)

## The call

**Keep klams as the memory store. Do not greenfield.**

The suspicion that a fresh start is warranted would be right if klams were
another prototype like krag/kris. It isn't. krag and kris are document-RAG
engines that never grew a memory model, an MCP surface, or a deployment; klams
is a deployed memory *service* that already implements essentially every line
of the WI #259 target state:

- MCP server with scoped tokens, live at `kubs0:7777/mcp`, consumed by VS Code
  today.
- A real memory model: facts/events/knowledge, author attribution, trust
  hierarchy, decay.
- Contradiction handling that is the exact "solve when contradicting memories
  are saved" workflow: conflicting lower-trust writes divert to **dissents**,
  and the viewport's `/dissents` page reviews, diffs, and promotes/discards
  them.
- Ops maturity a greenfield would take months to reach: systemd, backups with
  retention and lockfile recovery, Prometheus/Grafana, soak-tested connection
  handling, hourly ingestion of `~/src` + `~/obsidian`.

Meanwhile the two motivating stack changes turn out to be **edge concerns** for
klams, not core ones:

1. **vLLM** replaces how models are *served*. klams already treats serving as
   an external HTTP dependency; exactly two files speak model-server dialects
   (`TeiEmbedder`, `OllamaClient`). That's a small refactor, not a rewrite.
2. **LangChain** replaces hand-rolled *orchestration*. klams barely has any —
   its LLM usage is one optional summarization call. The hand-rolled
   orchestration that LangChain should replace lived in krag and kris (LLM
   pools, VRAM managers, synthesis pipelines), and those are already retired.
   LangChain's home in this architecture is the layer *around* the store:
   agents and pipelines that read/write memories over MCP/REST.

The greenfield math doesn't close: a Python/LangChain rewrite would spend its
first several sprints rebuilding dissents, attribution, scoped MCP auth, the
viewport, and the ops story — none of which LangChain provides — to reach the
state klams is in now, while carrying kris's documented failure mode
(infrastructure scope escalation before intelligence features). And this exact
decision already ran once: krag's MCP handoff was superseded *by* klams for
precisely these reasons.

## What greenfield energy should buy instead

The genuinely new work — the part that deserves fresh-project excitement — is a
**Python companion service** (working name: `klams-mind`) built on LangChain +
vLLM, talking to klams over its existing MCP/REST contract:

- **Memory extraction:** distill facts from Claude/GHCP session logs and
  conversations → `memory_add` with proper attribution.
- **Semantic contradiction detection:** klams's dissent trigger is
  trust-rank + same-fact conflict today; an LLM pass can *propose* dissents for
  semantically contradictory facts it wouldn't catch structurally.
- **Consolidation:** periodic merge/summarize/prune passes over aging
  memories (decay already provides the ranking signal).
- **Retrieval-quality evals:** port krag's eval-harness concept (TOML query
  suites, `source_cited` / `no_hallucination` checks) and run it against
  `memory_search` — klams has perf benches but no quality evals.

This gets all the LangChain learning value with zero rewrite risk, and it's
severable: if the Rust core ever becomes the constraint, `klams-mind` plus
klams's schema and MCP contract *are* the seed and spec of a successor.

## Proposed sprint 013 — "Serving pivot" (the klams-side work)

1. **Introduce an `Embedder` trait** in `klams-store`; make
   `CompositeStore.embedder` a `Box<dyn Embedder>`. Keep `TeiEmbedder` as one
   impl, add `OpenAiCompatEmbedder` (`POST /v1/embeddings`, OpenAI response
   shape) pointing at vLLM.
2. **Make embedding dimension config-driven** (Qdrant collection dim +
   `expected_dim` checks), and write the **re-embed migration** path (new
   collection, backfill from source, cut over) — needed once for any model
   upgrade, so do it while touching this code.
3. **Replace `OllamaClient`** with an OpenAI-compatible chat client
   (`/v1/chat/completions`) in `summarize/llm.rs`. Small file, graceful
   fallback already exists.
4. **Decide the embedding-serving topology** (see open question below).
5. Ride-alongs while in the area: D-004 filter pushdown, open bugs #31–#33.

Everything else in klams — schema, MCP, viewport, ingestion, ops — is
untouched by the stack pivot.

## Open questions to settle first

- **Where do embeddings run?** WI #259 says kubs0's 5090 handles light
  inference/embedding, but `kvllm` lives on kai. Options:
  (a) keep TEI on kubs0 (GPU image) — zero cross-machine dependency in the
  write path, and TEI is arguably the right tool for a small embedding model;
  (b) run a second small vLLM instance on kubs0; (c) point klams at kvllm on
  kai — simplest consolidation but makes the memory store's ingestion depend on
  another machine being up. Recommendation: (a) or (b) — keep the store
  self-contained on kubs0; use kai/kvllm for the heavier `klams-mind` LLM calls,
  which degrade gracefully.
- **Which embedding model** for the re-embed (current is 384-dim; krag/kris
  standardized on bge-base 768-dim; newer options exist) — pick once, since the
  migration cost is per-change.
- **Viewport reach:** the Tauri desktop app works, but if browsing memories
  from more devices matters, a browser-served mode is a future sprint — not
  part of this decision.

## Disposition of the other projects

- **krag:** archive (read-only). Before archiving, harvest: eval harness
  design, prompt presets, `response-tuning/findings.md` lessons. Its own docs
  already record the supersession.
- **kris:** archive (read-only). Harvest the design docs and the
  scope-escalation lesson; merge or delete the dangling
  `003-opensearch-transition` branch so the repo isn't left mid-flight.
- Record the disposition in korg WI #259 so the lineage is discoverable later.

## When to revisit this call

Greenfield becomes the right answer if any of these hold in ~2 quarters:
- The store itself (not its consumers) needs rapid LLM-native feature iteration
  and the Rust boundary is demonstrably the drag — i.e. `klams-mind` keeps
  wanting to reach *inside* klams rather than talk to its API.
- The memory model needs a structural rework (e.g. graph memory / TMX-style
  relations) that would touch most of the schema anyway.
- Off-the-shelf memory stores (mem0, Zep, Letta, LangGraph Store) mature to the
  point of covering the dissent/trust/attribution model — worth a periodic
  check, since "polished and maintained beats hand-rolled" is the same argument
  that motivated vLLM/LangChain.
