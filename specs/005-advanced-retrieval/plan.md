# Implementation Plan: Advanced Retrieval and Summarization

**Branch**: `005-advanced-retrieval` | **Date**: 2026-05-20 | **Spec**: [spec.md](spec.md)  
**Input**: Feature specification from `/specs/005-advanced-retrieval/spec.md`

## Summary

Phase 4 of the master plan ships the read-side upgrade for klams: a new
`POST /memory/context` endpoint that returns a deduped, budget-respecting
bundle of facts + knowledge + recent events; hybrid retrieval (vector +
Postgres FTS + metadata filters) fused via Reciprocal Rank Fusion (RRF) by
default with optional weighted blending; a background summarization task
that runs extractive summarization first and falls back to a local LLM
(Phi-3-medium via Ollama on `kubs0`'s GPU) when extractive output is
inadequate; and a viewport context-preview pane. Token cost uses
`tiktoken` cl100k_base with a `chars/4` fallback. Per-type decay
parameters already live in `[decay.lambda]`; this sprint hardens that
path with start-time validation and an effective-config log line, then
documents it.

## Technical Context

**Language/Version**: Rust 1.83 (workspace toolchain), Svelte 5 + Tauri 2 for viewport  
**Primary Dependencies**:
- Existing: `axum`, `tokio`, `sqlx` (Postgres), `qdrant-client`, `reqwest` (TEI), `tracing`, `metrics`, `serde`, `clap`
- New (this sprint): `tiktoken-rs` (cl100k_base encoder), direct `reqwest` to Ollama for the LLM fallback (no new SDK dep)

**Storage**: Postgres 16 (`facts`, `events`, plus a new `summaries` table for `EventSummary`); Qdrant (existing `knowledge_items` collection + a `kind=digest` flag for `KnowledgeDigest`)  
**Testing**: `cargo test` (unit + contract); `klams-service/tests/*` integration tests against `docker-compose.test.yml`; viewport: existing Vitest + Playwright harness  
**Target Platform**: Linux server on `kubs0` (managed by `klams-service.service` systemd unit); viewport runs on Windows via Tauri  
**Project Type**: Single project — Rust workspace under `crates/` plus `viewport/` (Tauri/Svelte) plus `migrations/`. No new top-level layout.  
**Performance Goals**:
- `/memory/context` p95 ≤ 2× current `/memory/search` p95 at budget=4 000 (SC-003)
- Hybrid query plan p95 ≤ 2× vector-only on the same query set (US2 acceptance #3)
- Summarization task can digest a 1 000-event cluster + 100-chunk knowledge cluster within one task cycle on `kubs0`'s default config (SC-004)

**Constraints**:
- Reuse the FTS `tsvector` index and JSONB `jsonb_path_ops` GIN index added in sprint 003; introduce new indexes only if `EXPLAIN ANALYZE` flags a regression (FR-006)
- LLM fallback MUST be optional — service runs cleanly when Ollama is down (FR-010)
- `/memory/search` response shape unchanged (FR-012); only ranking improves
- Token-budget accounting must report which encoder produced the count (FR-002 v2)
- Constitution gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, docs updated

**Scale/Scope**:
- Target: ≥ 1 000 facts, ≥ 5 000 knowledge chunks, ≥ 10 000 events for SC-001 representative-query benchmark
- Live data: ansible-k pushes ~21 EnvFacts; obsidian/repo scanner has not been run at scale yet — synthetic fixtures will fill the gap for the bench
- New code surface estimate: 1 endpoint, 1 background task, 1 viewport pane, ~3 new modules in `klams-core` (`hybrid`, `context`, `summarize`), 1 migration

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

Constitution version: 1.0.0 (ratified 2026-05-16).

| Principle | Compliance plan |
|---|---|
| **I. Spec-Driven Development** | Spec already at [spec.md](spec.md). All work tracks against numbered FRs and SCs. Mid-sprint discoveries land as spec amendments, not code-only changes. |
| **II. Test-Driven Development** | Each FR maps to one or more tests written first: contract tests under `crates/klams-api/tests/contract_*.rs`, integration tests under `crates/klams-service/tests/`, unit tests inline. RRF and token-budget allocation get pure-function unit tests in `klams-core`. |
| **III. Code Standards Gate** | `just check` runs the full pre-commit gate before each commit and at PR boundary. New crates pin clippy-clean (`-D warnings`). |
| **IV. Documentation** | This sprint touches user-visible surface (new endpoint, new config keys, viewport pane). `docs/architecture.md` (hybrid + summarization sections), `docs/usage.md` (`/memory/context` example, decay tuning), and `deploy/config/klams.example.toml` (LLM/summarization keys, RRF k) are part of the DoD. |
| **V. Quality & Observability** | New Prometheus metrics per FR-014 (`/memory/context` latency histogram, hybrid per-source counters, summarization run counters, decay-config reload events). Errors are actionable — invalid decay config refuses startup with the offending key named (US4 acceptance #2). `/memory/context` uses per-section status to degrade gracefully on store outage (FR-011). |
| **VI. Simplicity & Intentional Design** | RRF chosen over weighted blending as default (parameter-light, no normalization). Cross-encoder rerank explicitly out of scope. Summarization is extractive-first with LLM fallback only when extractive fails — no upfront LLM dependency on the hot path. No new data store. No reload mechanism beyond restart in this sprint (defer SIGHUP/file-watch to backlog). YAGNI on per-section budget overrides — ship only `token_budget` plus an automatic floor per non-empty section. |

**Gate result**: PASS. No violations to track.

## Project Structure

### Documentation (this feature)

```text
specs/005-advanced-retrieval/
├── spec.md              # Feature specification (already written)
├── plan.md              # This file (/speckit.plan output)
├── research.md          # Phase 0 output (/speckit.plan output)
├── data-model.md        # Phase 1 output (/speckit.plan output)
├── quickstart.md        # Phase 1 output (/speckit.plan output)
├── contracts/
│   └── memory-context.openapi.yaml   # Phase 1 output
├── checklists/
│   └── requirements.md  # /speckit.specify output
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
crates/
├── klams-types/             # +ContextRequest, ContextBundle, FusionStrategy, SummaryKind, ConfigError types
├── klams-core/
│   ├── src/
│   │   ├── hybrid.rs        # NEW: RRF + weighted blending fusion; HybridQueryPlan
│   │   ├── context.rs       # NEW: ContextBuilder — query plan -> bundle, dedupe, budget fit
│   │   ├── summarize/
│   │   │   ├── mod.rs       # NEW: SummarizationTask; scheduling; cluster detection
│   │   │   ├── extractive.rs# NEW: rule-based event headlines + chunk excerpting
│   │   │   └── llm.rs       # NEW: Ollama HTTP client (Phi-3-medium); off when disabled
│   │   ├── tokens.rs        # NEW: TokenCounter (tiktoken cl100k_base + chars/4 fallback)
│   │   ├── decay.rs         # MODIFIED: validate_loaded_config(); effective-config log line
│   │   └── lib.rs           # re-exports
├── klams-store/
│   ├── src/
│   │   ├── postgres.rs      # MODIFIED: filtered FTS variants (host/type/since); summaries CRUD
│   │   ├── qdrant.rs        # MODIFIED: digest-aware filtering (kind=digest), filtered vector search
│   │   └── lib.rs           # MODIFIED: new traits HybridStore + SummaryStore
├── klams-api/
│   ├── src/
│   │   ├── routes/
│   │   │   └── context.rs   # NEW: POST /memory/context handler
│   │   └── routes/search.rs # MODIFIED: use HybridStore for search ranking; same response shape
│   └── tests/
│       └── contract_context.rs # NEW
└── klams-service/
    └── tests/
        ├── phase4_context_bundle.rs           # NEW (US1)
        ├── phase4_hybrid_retrieval.rs         # NEW (US2)
        ├── phase4_summarization_pipeline.rs   # NEW (US3)
        └── phase4_decay_config_validation.rs  # NEW (US4)

migrations/
└── 0004_summaries.sql               # NEW: summaries table; covering indexes if EXPLAIN demands

deploy/config/
└── klams.example.toml               # MODIFIED: [retrieval], [summarization], [tokens] blocks; uncomment [decay.lambda] example

viewport/
├── src/lib/components/
│   └── ContextPreview.svelte        # NEW: query box + budget slider + bundle render
├── src/lib/api/
│   └── context.ts                   # NEW: typed client for /memory/context

docs/
├── architecture.md                  # MODIFIED: hybrid retrieval, summarization, context endpoint sections
├── usage.md                         # MODIFIED: /memory/context examples; decay tuning recipe
└── viewport.md                      # MODIFIED: §6 Phase 4 context-preview pane fleshed out
```

**Structure Decision**: Existing single-project Rust workspace (8 crates under
`crates/`) plus the Tauri/Svelte viewport (`viewport/`) plus Postgres
migrations under `migrations/`. Phase 4 adds modules and one migration; no
new crates. The `klams-mcp` crate is still deferred to Phase 6.

## Complexity Tracking

> **Fill ONLY if Constitution Check has violations that must be justified**

No violations. Section intentionally empty.

## Phase 0 Output (Research)

See [research.md](research.md). All `NEEDS CLARIFICATION` markers from the
spec were resolved by the user before this plan was written; Phase 0
records the chosen approaches plus residual planning-level decisions (RRF
`k`, event-cluster definition, stale thresholds, LLM prompt template,
summary storage location, viewport SSE-vs-poll, Ollama provisioning).

## Phase 1 Output (Design & Contracts)

- [data-model.md](data-model.md) — `ContextBundle`, `EventSummary`,
  `KnowledgeDigest`, `DecayConfig` (existing + validation),
  `HybridQueryPlan`, `Summaries` table schema.
- [contracts/memory-context.openapi.yaml](contracts/memory-context.openapi.yaml) —
  `POST /memory/context` request/response schema; degraded-section semantics.
- [quickstart.md](quickstart.md) — end-to-end demo: configure, restart,
  `curl /memory/context`, observe metrics, open viewport pane.
- Agent context updated: the SPECKIT marker in
  `.github/copilot-instructions.md` now points to this plan.
