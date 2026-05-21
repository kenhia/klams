# Tasks: Advanced Retrieval and Summarization

**Input**: Design documents from `/specs/005-advanced-retrieval/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/memory-context.openapi.yaml, quickstart.md

**Tests**: Included. Per the project constitution (Principle II) and plan §Constitution Check, every FR maps to one or more tests written first.

**Organization**: Tasks are grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story?] Description`

- **[P]**: Different files, no dependencies on incomplete tasks — safe to run in parallel
- **[Story]**: User story label (US1..US5); omitted in Setup, Foundational, and Polish phases

## Path Conventions

Single Rust workspace at repo root. Crates under `crates/`, viewport under `viewport/`, migrations under `migrations/`, docs under `docs/`, deploy config under `deploy/config/`. All paths shown are relative to the repo root.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Add dependencies, scaffold new module files, prepare config example.

- [X] T001 Add `tiktoken-rs` to `crates/klams-core/Cargo.toml` (pick the latest 0.x via `cargo search tiktoken-rs`; pin to the chosen `major.minor`). Add a smoke test in `crates/klams-core/src/tokens.rs` (gated `#[test]`) that calls `cl100k_base()` and asserts a non-zero token count for `"hello world"` — this catches missing-data-file regressions at compile/test time, not at startup.
- [X] T002 [P] Create empty module files with `mod` stubs: `crates/klams-core/src/hybrid.rs`, `crates/klams-core/src/context.rs`, `crates/klams-core/src/tokens.rs`, `crates/klams-core/src/summarize/mod.rs`, `crates/klams-core/src/summarize/extractive.rs`, `crates/klams-core/src/summarize/llm.rs`. Wire them into `crates/klams-core/src/lib.rs`.
- [X] T003 [P] Create `crates/klams-api/src/routes/context.rs` stub (handler placeholder returning `501 Not Implemented`) and register the route in `crates/klams-api/src/routes/mod.rs`.
- [X] T004 [P] Add new TOML blocks (`[retrieval]`, `[tokens]`, `[summarization]`) to `deploy/config/klams.example.toml` with the values from data-model.md §8; uncomment the `[decay.lambda]` example block.

**Checkpoint**: Workspace compiles with new module stubs; example config carries the new keys.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared types, the new migration, and store traits — every user story depends on these.

**⚠️ CRITICAL**: No user story work begins until this phase is complete.

- [X] T005 Create migration `migrations/0004_summaries.sql` per data-model.md §3 — table `summaries` with constraints, plus indexes `summaries_day_bucket_idx`, `summaries_category_idx`, `summaries_invalidated_idx`. Verify it applies cleanly to a fresh database.
- [X] T006 [P] Add request/response types to `crates/klams-types/src/context.rs` (NEW): `ContextRequest`, `RetrievalFilters`, `ContextBundle`, `ContextItem`, `SectionMeta`, `SectionSource`, `SectionStatus`, `ItemKind`, `TokenEncoderId`. Re-export from `crates/klams-types/src/lib.rs`. Match the shapes in data-model.md §1–§2 and §7.
- [X] T007 [P] Add hybrid-retrieval types to `crates/klams-types/src/retrieval.rs` (NEW): `HybridQueryPlan`, `FusionStrategy` (`Rrf { k }`, `Weighted { vector, fts, normalization }`), `WeightedNorm`, `RetrievalSource` enum. Re-export from `crates/klams-types/src/lib.rs`.
- [X] T008 [P] Add summary types to `crates/klams-types/src/summary.rs` (NEW): `EventSummary` struct mirroring the `summaries` row, `KnowledgeDigest` struct mirroring the Qdrant payload (data-model.md §3 and §4), plus `SummaryMechanism` enum.
- [X] T009 [P] Extend the existing `ConfigError` enum in `crates/klams-service/src/config.rs` with the new variants from data-model.md §9 (`DecayLambdaNegative`, `DecayLambdaNonFinite`, `DecayUnknownType`, `RetrievalFusionUnknown`, `SummarizationOllamaUrlInvalid`).
- [X] T010 Define traits `HybridStore` and `SummaryStore` in `crates/klams-store/src/lib.rs` (method signatures only, with `async_trait`); concrete implementations come in story phases.
- [X] T011 [P] Extend the existing `Config` struct in `crates/klams-service/src/config.rs` with `RetrievalConfig`, `TokensConfig`, and `SummarizationConfig` sub-structs deserializing the new `[retrieval]`, `[tokens]`, `[summarization]` blocks. No semantic validation yet — that arrives in US4.
- [X] T012 Confirm `cargo check --workspace` is green after T005–T011.

**Checkpoint**: Foundation ready — user-story phases may proceed.

---

## Phase 3: User Story 1 — Agents fetch a coherent context bundle (Priority: P1) 🎯 MVP

**Goal**: `POST /memory/context` returns a deduped, budget-respecting bundle of `facts[] / knowledge[] / events[]` for a representative query (FR-001, FR-002, FR-003, FR-004, FR-011).

**Independent Test**: With ≥ 1 fact / ≥ 1 knowledge chunk / ≥ 0 events that match a representative query, `POST /memory/context { query, token_budget: 4000 }` returns a bundle with `total_spent ≤ 4000`, no item appearing in two sections, and `truncated` correctly set. Re-running at `token_budget: 1000` yields a strictly smaller subset of the same items.

### Tests for User Story 1 (TDD — write first, watch fail) ⚠️

- [ ] T013 [P] [US1] Contract test in `crates/klams-api/tests/contract_context.rs` validating request/response against `specs/005-advanced-retrieval/contracts/memory-context.openapi.yaml`. Cover: happy path; empty-budget probe; missing query → 4xx with `query_required`; **unknown filter key → 4xx with the offending key named** (edge-case bullet from spec); unhealthy-store → 200 with degraded section.
- [ ] T014 [P] [US1] Unit tests in `crates/klams-core/src/context.rs` for the budget-fitter: minimum-floor-per-section rule, dedupe precedence (fact > knowledge > event for structured attrs; knowledge > events for prose), `truncated` bookkeeping, single-item-larger-than-budget edge case.
- [ ] T015 [P] [US1] Unit tests in `crates/klams-core/src/tokens.rs` for the `TokenCounter`: cl100k_base path, fallback path, `which_encoder()` reports active mode, large-payload safety.
- [ ] T016 [US1] Integration test in `crates/klams-service/tests/phase4_context_bundle.rs` exercising the full HTTP path against `docker-compose.test.yml` — uses a known fixture set, asserts items per the Independent Test above.

### Implementation for User Story 1

- [ ] T017 [P] [US1] Implement `TokenCounter` in `crates/klams-core/src/tokens.rs`: `cl100k_base` via `tiktoken-rs::cl100k_base()`; fallback `chars / 4`; mode selected by `[tokens] mode`; expose `count(&str) -> u32` and `encoder_id() -> TokenEncoderId`.
- [ ] T018 [US1] Implement `ContextBuilder` in `crates/klams-core/src/context.rs`: takes a `ContextRequest`, calls a (single-source for now — vector-only) `HybridStore`, applies dedupe rules (FR-004) and budget allocation (FR-003), produces a `ContextBundle`. Per-section status reflects store availability (FR-011).
- [ ] T019 [P] [US1] Implement vector-only `HybridStore` impl in `crates/klams-store/src/qdrant.rs` so US1 can ship before US2 lands the full hybrid path. Returns ranked items with placeholder source attribution.
- [ ] T020 [US1] Wire `POST /memory/context` handler in `crates/klams-api/src/routes/context.rs`: deserialize `ContextRequest`, enforce auth (existing bearer middleware), invoke `ContextBuilder`, serialize `ContextBundle`. Return 4xx with `query_required` on empty query; 503 with `Retry-After` only when *all* sources are unavailable.
- [ ] T021 [US1] Add `klams_context_request_seconds` histogram and `klams_context_section_items_total` counter in the handler (FR-014).

**Checkpoint**: US1 contract test green; manual `curl` of `/memory/context` returns a populated bundle within budget.

---

## Phase 4: User Story 2 — Hybrid retrieval finds matches that pure vector search misses (Priority: P1)

**Goal**: Vector + Postgres FTS + metadata filters fused via RRF (default) or weighted blending (opt-in); `/memory/search` and `/memory/context` both consume it (FR-005, FR-006, FR-012).

**Independent Test**: Index a fact whose payload contains `cuda_toolkit_version=12.4` and a knowledge note that paraphrases it without those literals. Query `cuda 12.4` ranks the fact first; query `nvidia toolkit homelab` ranks the note first. Both queries return both items. `EXPLAIN ANALYZE` shows the FTS + JSONB GIN indexes are used.

### Tests for User Story 2 ⚠️

- [ ] T022 [P] [US2] Unit tests in `crates/klams-core/src/hybrid.rs` for RRF fusion: monotonic in rank, identical inputs yield identical scores, k-parameter scaling, empty source handling, weighted-blending normalization (z-score and min-max).
- [ ] T023 [P] [US2] Integration test in `crates/klams-service/tests/phase4_hybrid_retrieval.rs` covering the literal vs paraphrase scenario from the Independent Test plus filter pre-pruning (`host=kubs0`, `since=7d`) and the "one source returns zero rows" edge case.
- [ ] T024 [P] [US2] Performance check in the same integration file: hybrid p95 ≤ 2× vector-only on a ≥ 10 000-row fixture (run `EXPLAIN ANALYZE` and assert it references the FTS and `jsonb_path_ops` GIN indexes).

### Implementation for User Story 2

- [ ] T025 [P] [US2] Implement RRF and weighted fusion in `crates/klams-core/src/hybrid.rs`. Pure functions over `Vec<RankedRow>` per source; no I/O.
- [ ] T026 [US2] Implement filtered FTS retrieval in `crates/klams-store/src/postgres.rs`: parameterized variants over `facts` and `knowledge_chunks` reusing the sprint-003 `tsvector` index; bind metadata filters (host, type, since/until) via the JSONB `jsonb_path_ops` GIN index. Confirm with `EXPLAIN ANALYZE`.
- [ ] T027 [US2] Implement filtered vector retrieval in `crates/klams-store/src/qdrant.rs`: payload filter for host/type/since plus `kind != "digest"` when callers ask for raw only.
- [ ] T028 [US2] Replace the vector-only `HybridStore` impl from T019 with a real fan-out: per-source `top_k` (capped via `[retrieval] per_source_top_k`), then fuse via the configured `FusionStrategy`.
- [ ] T029 [US2] Modify `crates/klams-api/src/routes/search.rs` to call the new `HybridStore` path; response shape MUST stay byte-compatible (FR-012). Confirm against the existing `/memory/search` contract test.
- [ ] T030 [US2] Add `klams_hybrid_source_hits_total{source}` counter in the hybrid path (FR-014).

**Checkpoint**: US2 integration test green; `/memory/search` and `/memory/context` both demonstrably surface paraphrase + literal matches; `EXPLAIN ANALYZE` confirms the indexes are used.

---

## Phase 5: User Story 3 — Background summarization (Priority: P2)

**Goal**: A scheduled task produces `EventSummary` rows and `KnowledgeDigest` Qdrant entries; the retrieval path picks raw vs summary based on budget headroom (FR-008, FR-009, FR-010).

**Independent Test**: Insert 200 `service.up` events for `qdrant` on `kubs0` over 14 days. Run one summarization cycle. A `summaries` row exists for the cluster. A `/memory/context` query whose ranking includes those events at `token_budget = 200` substitutes the summary record (with `kind: "summary"` and `source_count: 200`) instead of raw events.

### Tests for User Story 3 ⚠️

- [ ] T031 [P] [US3] Unit tests in `crates/klams-core/src/summarize/extractive.rs` for event headlines (top-K category counts, time-bracket phrasing) and chunk excerpting (longest-representative selection).
- [ ] T032 [P] [US3] Unit tests in `crates/klams-core/src/summarize/llm.rs` against a `wiremock`-style stub for Ollama: success path, model-missing fallback, network-error fallback (LLM disabled mid-cycle).
- [ ] T033 [P] [US3] Integration test in `crates/klams-service/tests/phase4_summarization_pipeline.rs` covering: (a) the Independent Test scenario end-to-end against `docker-compose.test.yml` + a stub Ollama (or `llm_fallback = false`); (b) **summarization disabled** (`[summarization] enabled = false`) — `/memory/context` still returns a non-empty bundle within budget using raw items only (closes SC-007); (c) **invalidation fallback** — after a summary is written and then `invalidated_at` is set, the next `/memory/context` call for the same query returns raw events for that section instead of the summary.

### Implementation for User Story 3

- [ ] T034 [P] [US3] Implement extractive summarization in `crates/klams-core/src/summarize/extractive.rs` per research.md D-005 and D-006: cluster detection on `(host, category, day_bucket)` for events and `(repo, file_prefix(2))` for stale knowledge.
- [ ] T035 [P] [US3] Implement Ollama HTTP client in `crates/klams-core/src/summarize/llm.rs`: direct `reqwest` POST to `[summarization] ollama_url`; one-shot probe at task start that disables fallback for the cycle on failure (research.md D-010).
- [ ] T036 [US3] Implement `SummarizationTask` in `crates/klams-core/src/summarize/mod.rs`: scheduled at `[summarization] task_interval_seconds`; cycles do not lap (mutex/run-flag); writes `EventSummary` via `SummaryStore` and `KnowledgeDigest` via `qdrant.rs`; sets `mechanism` per record.
- [ ] T037 [US3] Implement `SummaryStore` impl in `crates/klams-store/src/postgres.rs`: insert/upsert by `(kind, host, category, day_bucket)`, invalidate by setting `invalidated_at`, list by cluster, fetch by id.
- [ ] T038 [US3] Extend the digest path in `crates/klams-store/src/qdrant.rs`: write digests with `kind = "digest"` payload; retrieval filters `invalidated_at = null` and respects callers' raw-vs-digest preference.
- [ ] T039 [US3] Wire summary substitution into `ContextBuilder` (`crates/klams-core/src/context.rs`): when raw items would blow a section's allowed budget, swap in the matching summary record (`source_count`, `source_ids`).
- [ ] T040 [US3] Register `SummarizationTask` in `crates/klams-service/src/main.rs` (or wherever the decay task is started); guard on `[summarization] enabled`.
- [ ] T041 [US3] Add metrics `klams_summarization_runs_total{mechanism}` and `klams_summarization_lag_seconds` (FR-014).

**Checkpoint**: US3 integration test green; the Independent Test scenario produces and surfaces a summary; service starts cleanly when Ollama is unreachable.

---

## Phase 6: User Story 4 — Decay parameters move from code to config (Priority: P2)

**Goal**: `DecayConfig::validate()` runs at startup; bad config refuses to start with the offending key; on success, an `INFO` line records the effective per-type table (FR-007).

**Independent Test**: Set obviously-different per-type λ values in `klams.toml`, restart the service. A query whose top results span types of different ages reorders accordingly. An invalid value (negative λ, unknown type) refuses startup with an actionable error.

### Tests for User Story 4 ⚠️

- [ ] T042 [P] [US4] Unit tests in `crates/klams-types/src/decay.rs` for `DecayConfig::validate()`: negative λ, non-finite λ, unknown type key, all-default empty map (allowed), happy-path map.
- [ ] T043 [P] [US4] Integration test in `crates/klams-service/tests/phase4_decay_config_validation.rs`: invalid TOML → service exits non-zero with the offending key in stderr; valid TOML → service starts and `journalctl` (or test log capture) shows the `decay config loaded:` line; tuning λ measurably reorders a fixture query (per Independent Test).

### Implementation for User Story 4

- [ ] T044 [US4] Implement `DecayConfig::validate(&self) -> Result<(), DecayConfigError>` in `crates/klams-types/src/decay.rs` per data-model.md §5.
- [ ] T045 [US4] Call `validate()` in service startup (e.g. `crates/klams-service/src/main.rs`); on `Err`, log + exit non-zero. On `Ok`, emit one `INFO` line: `decay config loaded: <type>=<lambda> ... interval=<n>s batch=<n>`.
- [ ] T046 [US4] Add `klams_decay_config_reload_total` counter incremented once at successful startup load (FR-014); document that no SIGHUP reload is in scope this sprint (matches research.md D-007).

**Checkpoint**: US4 integration test green; `klams.example.toml` shows a working `[decay.lambda]` block.

---

## Phase 7: User Story 5 — Viewport context-preview pane (Priority: P2)

**Goal**: A new viewport pane lets Ken eyeball-validate `/memory/context` output for a query at varying budgets, with raw-vs-summarized toggle (FR-013).

**Independent Test**: Open the viewport, type a representative query, slide the budget from max down to a tight value, observe per-section token counts updating; toggle raw-vs-summarized and observe the events section flip between raw rows and a summary record.

### Tests for User Story 5 ⚠️

- [ ] T047 [P] [US5] Vitest component test for `viewport/src/lib/components/ContextPreview.svelte`: renders bundle sections, slider triggers debounced `POST /memory/context` (250 ms — research.md D-009), toggle re-fetches without losing query/budget state.
- [ ] T048 [P] [US5] Vitest unit test for the typed client in `viewport/src/lib/api/context.ts`: encodes `ContextRequest` correctly, decodes degraded sections, surfaces 503 + `Retry-After`.

### Implementation for User Story 5

- [ ] T049 [P] [US5] Create typed client `viewport/src/lib/api/context.ts` (request type, response type, fetch helper using the existing auth/base-URL plumbing).
- [ ] T050 [US5] Create `viewport/src/lib/components/ContextPreview.svelte`: query box, token-budget slider (debounced 250 ms), per-section render with token-count readouts, raw-vs-summarized toggle, error state on store unreachable.
- [ ] T051 [US5] Wire the new pane into the existing viewport navigation (`viewport/src/lib/components/Sidebar.svelte` or equivalent) so the pane is reachable at `/preview` (or whichever route convention the viewport uses).

**Checkpoint**: `pnpm tauri dev` shows the new pane working against a live klams-service; SC-006 demonstrably met.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, the SC-001/SC-003/SC-004 benchmarks, and the quickstart pass.

- [ ] T052 [P] Update `docs/architecture.md` with hybrid-retrieval, summarization, and `/memory/context` sections (FR-015).
- [ ] T053 [P] Update `docs/usage.md` with `/memory/context` request/response examples and a decay-tuning recipe (FR-015).
- [ ] T054 [P] Update `docs/viewport.md` §6 with the new context-preview pane.
- [ ] T055 Run the SC-001 / SC-003 benchmark on a populated fixture (≥ 1 000 facts, ≥ 5 000 knowledge chunks, ≥ 10 000 events). Record p95 latencies and budget-conformance numbers in `specs/005-advanced-retrieval/quickstart.md` "Validation" section.
- [ ] T056 Run the SC-004 summarization benchmark (1 000-event cluster, 100-chunk cluster, single cycle, no lap). Record numbers in the quickstart.
- [ ] T057 [P] Run `just check` (fmt, clippy `-D warnings`, test workspace, viewport build) — must be green before merge.
- [ ] T058 Run end-to-end through `specs/005-advanced-retrieval/quickstart.md`. Tick each acceptance bullet.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 Setup**: no dependencies — start immediately.
- **Phase 2 Foundational**: depends on Phase 1; **blocks every user story**.
- **Phase 3 US1 (P1)**: depends on Phase 2.
- **Phase 4 US2 (P1)**: depends on Phase 2; can start in parallel with US1, but T028 swaps in for T019, so US2 lands cleanly after US1 if both touch `qdrant.rs`.
- **Phase 5 US3 (P2)**: depends on Phase 2; benefits from US1's `ContextBuilder` (T039 modifies it) and US2's `HybridStore` for ranking digests.
- **Phase 6 US4 (P2)**: depends only on Phase 2 — fully independent of other stories.
- **Phase 7 US5 (P2)**: depends on US1 endpoint existing (T020).
- **Phase 8 Polish**: depends on every desired user story.

### Within Each Story

- Tests written first, observed to fail, then implementation.
- Types/models → store impls → service logic → handler/UI → metrics.

### Parallel Opportunities

- T002, T003, T004 (Setup) run in parallel.
- T006–T011 (Foundational types/traits) all `[P]` — different files.
- US1 and US2 contract/unit tests (`T013–T015`, `T022`) all `[P]`.
- US4 is fully independent of US1/US2/US3 — can be picked up by anyone at any time after Phase 2.
- Polish doc tasks T052–T054 all `[P]`.

### MVP Scope

User Story 1 alone delivers the demonstrable exit criterion ("`/memory/context` returns a coherent bundle under a configurable token budget for a representative query"), with vector-only retrieval and raw-only items. US2 sharpens recall. US3 enables the budget-fit story for large match sets. US4 makes decay tunable. US5 makes the demo human-friendly. **Recommended MVP cut: T001–T021 (Setup + Foundational + US1).**

### Suggested Increment Order

1. Setup + Foundational (T001–T012) — one PR.
2. US1 (T013–T021) — MVP increment, ships `/memory/context`.
3. US2 (T022–T030) — recall increment, lifts both endpoints.
4. US4 (T042–T046) — config-tuning increment (small, independent).
5. US3 (T031–T041) — summarization increment.
6. US5 (T047–T051) — UI increment.
7. Polish (T052–T058) — docs + benches + quickstart pass.

---

## Task Count Summary

| Phase | Count | Notes |
|---|---|---|
| Setup | 4 | T001–T004 |
| Foundational | 8 | T005–T012 |
| US1 (P1) | 9 | T013–T021 (4 tests + 5 impl) |
| US2 (P1) | 9 | T022–T030 (3 tests + 6 impl) |
| US3 (P2) | 11 | T031–T041 (3 tests + 8 impl) |
| US4 (P2) | 5 | T042–T046 (2 tests + 3 impl) |
| US5 (P2) | 5 | T047–T051 (2 tests + 3 impl) |
| Polish | 7 | T052–T058 |
| **Total** | **58** | |
