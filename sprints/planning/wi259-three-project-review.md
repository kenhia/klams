# WI #259 — Three-project review: krag / kris / klams

**Date:** 2026-07-05
**Question:** Given the pivot to vLLM (serving, via `kvllm` on `kai`) and LangChain
(orchestration), should the homelab memory store continue on `klams` or start
greenfield?
**Target state (from WI #259):** memory store on `kubs0` (RTX 5090 for light
inference/embedding), accessible to agents (Claude, GHCP, local) via MCP, with a
UI to browse memories and resolve contradictions.

This document is the evidence. The recommendation lives in
[wi259-recommendation.md](wi259-recommendation.md).

---

## 1. Timeline & lineage

| Project | Language | Active | Sprints | Size | Status |
|---|---|---|---|---|---|
| `krag` | Python | ~Nov 2025 – mid 2026 | 15 | ~19k LOC src, ~30k LOC tests | Complete-ish, superseded |
| `kris` | Python + Rust scanner | 2026-03-18 → 2026-03-29 (12 days) | 3 of a planned 13 | ~4.3k LOC py + ~1k rs | Abandoned mid-migration; last branch never merged |
| `klams` | Rust | ~Apr 2026 → now | 12 merged | ~33k LOC rs + ~3.2k Svelte/TS | **Live in production on kubs0** |

Key historical fact: this keep-vs-replace decision has already been run once.
`krag`'s `sprints/planning/handoff-mv-rag-mcp.md` designed a full MCP facade for
krag, then was marked **SUPERSEDED** with the note that the consuming project
(multae-viae) *"chose klams as its RAG/memory backend instead (klams already
provides the MCP server, auth, retrieval, ingestion, and kubs0 deployment this
handoff planned to build)."*

---

## 2. Scorecard against the WI #259 target state

| Requirement | krag | kris | klams |
|---|---|---|---|
| Memory model (facts, not just documents) | ✗ document RAG only | ✗ document RAG only | ✓ facts / events / knowledge, three memory kinds |
| Contradiction handling | ✗ | ✗ | ✓ dissents: trust-ranked conflict diversion, `Pending → Promoted \| Discarded` state machine |
| UI to browse + resolve | Tauri GUI (krager), query-oriented, no memory concept | ✗ CLI only | ✓ viewport with `/dissents` resolution page, provenance panel, authors, activity feed |
| MCP server | ✗ (designed on paper, never built) | ✗ | ✓ `rmcp` streamable HTTP at `/mcp`, 10 tools, Read/Write/Admin scoped tokens, per-tool integration tests |
| Provenance / attribution | file-level source only | file-level only | ✓ authors table, per-write attribution, trust hierarchy (`User > Controller > Task > AgentProposal`), re-attribution repair tool |
| Ingestion of vault/repos | ✓ pull-scan, plugins, AST chunking | ✓ Rust scanner → task DAG | ✓ hourly systemd scanner (`~/src`, `~/obsidian`), gitignore-aware, delete-on-vanish |
| Deployed on kubs0 | ✗ | ✗ | ✓ systemd units, Docker Compose deps, backups w/ retention, Prometheus + Grafana, UFW-scoped LAN access |
| Retrieval | multi-model RRF fusion, boosting, modes | OpenSearch k-NN + filters | hybrid vector+FTS, RRF/weighted fusion, token-budgeted `/memory/context` bundler |

klams is the only one of the three that is actually a *memory system*; krag and
kris are document-RAG engines. The two capabilities WI #259 centers on —
MCP access and contradiction resolution with a UI — exist **only** in klams,
where they are built, tested, and deployed.

---

## 3. Fit against the new stack (vLLM + LangChain)

### 3.1 Where each project touches model serving today

- **krag:** in-process `sentence-transformers` + `llama-cpp-python` with a
  hand-rolled `LLMPool` (VRAM budgeting, hot-swap). Its research doc explicitly
  *rejected* OpenAI-compatible APIs, Ollama, and vLLM as "overkill." The entire
  serving layer is obsolete under the vLLM direction.
- **kris:** same story — local `sentence-transformers` + `llama-cpp-python`,
  plus a custom VRAM-aware ModelManager. All obsoleted by vLLM.
- **klams:** serving is **already out-of-process behind HTTP**, in exactly two
  files:
  - `crates/klams-store/src/embeddings.rs` — `TeiEmbedder` calling TEI's native
    `POST /embed` (not OpenAI-shaped); 384-dim baked into config, Qdrant
    collection, and `expected_dim` checks.
  - `crates/klams-core/src/summarize/llm.rs` — `OllamaClient` calling Ollama's
    native `/api/generate`; used **only** for optional summarization with
    graceful extractive fallback.

  Neither is behind a clean provider trait (`CompositeStore.embedder` is typed
  as the concrete `TeiEmbedder`), but the blast radius of an OpenAI-compatible
  swap is two small files + config + a one-time re-embed migration if the
  embedding model/dimension changes.

### 3.2 The LangChain question — the crux

LangChain is a Python orchestration framework. klams is a Rust *service*. These
do not compete; they meet at an API boundary. What LangChain actually provides
(chains, retrievers, loaders, agent orchestration, LLM clients) overlaps with
almost **none** of klams's 33k lines — klams's value is the memory model
(dissents, trust, decay, attribution), the MCP auth surface, and the ops story.
LangChain has no off-the-shelf equivalent for any of those; conversely, klams
deliberately keeps LLM calls at its edge (one summarization client).

The place LangChain earns its keep in this architecture is in the *consumers
and feeders* of the memory store: conversation-memory extraction, LLM-driven
contradiction detection, consolidation/summarization pipelines, agent
orchestration. All of those speak to klams over MCP/REST regardless of what
language klams is written in.

A greenfield Python rewrite would therefore spend most of its effort
re-implementing things LangChain does not provide (dissent state machine, scoped
MCP auth, viewport, attribution, decay, backups, systemd ops) in order to get
back to a state klams is already in.

### 3.3 Honest costs of staying on Rust/klams

- LLM-adjacent experimentation (new rerankers, embedding models, extraction
  prompts) is Python-first territory; iterating on that *inside* the Rust core
  would be slow. (Mitigation: don't — put it in a Python sidecar.)
- The 384-dim embedding assumption means any model upgrade forces a re-embed +
  Qdrant collection migration. This is a one-time cost that exists in *any*
  architecture, but klams should take the opportunity to make dim configurable.
- Known open debt: filter post-filtering with ×3 over-fetch instead of DB-side
  pushdown (D-004), no cross-encoder reranker, a few open viewport/tooling bugs
  (#31–#33), and 52 test files gated behind live-infra `#[ignore]`.
- README's own disclaimer: purpose-built, hostnames and paths baked in. For a
  single-operator homelab this is a feature more than a bug.

---

## 4. Salvage inventory (regardless of decision)

From **krag** (highest reuse value of the two retired projects):
- Evaluation harness design (`src/krag/evaluation/`) — TOML-defined query suites
  with `substring` / `source_cited` / `no_hallucination` checks. klams has a
  perf bench but **no retrieval-quality eval**; this concept should be ported.
- Prompt presets (`src/krag/synthesis/prompt_builder.py`) — four tuned presets
  incl. insufficient-context short-circuit; directly reusable in any
  LangChain pipeline.
- Retrieval lessons (`sprints/response-tuning/findings.md`): NL-trained embeddings
  rank docstrings over raw code constants; dedup before top-k or duplicates
  flood results; small local models confidently fabricate — cite-or-refuse
  prompting is mandatory.
- tree-sitter AST chunking (also present in kris) — relevant if code-aware
  memory ingestion ever matters.
- The MCP `rag_*` tool contract in `sprints/planning/handoff-mv-rag-mcp.md` §3 —
  useful design reference if klams ever grows a document-RAG-flavored tool.

From **kris**:
- Design docs are the asset: `docs/architecture.md`, `data-model.md`, the
  OpenSearch trade study (`docs/opensearch-research/`), and the
  content-addressed dedup model. Code salvage value is low; everything
  inference-related is obsoleted by vLLM.
- Cautionary lesson (worth writing down): kris died of **infrastructure scope
  escalation** — two consecutive plumbing sprints (hardening, then a
  Qdrant→OpenSearch backend swap) before any intelligence features shipped,
  then abandonment. A greenfield restart would face the same gravity well.

From **klams** (if greenfield were chosen anyway):
- The Postgres schema (8 migrations), `PublicMemory` projection boundary, the
  dissent/trust model, and the MCP tool contract would effectively *be* the spec
  for the successor — which is itself an argument that the successor already
  exists.
