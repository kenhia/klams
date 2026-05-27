# Tasks: Activity & Observability

**Feature**: 008-activity-observability
**Input**: Design documents from `/specs/008-activity-observability/`
**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: REQUIRED. Sprint 007 set the bar with integration tests per phase; US5 explicitly requires a measurable contract. Integration tests are written alongside (not before) implementation per existing klams house style.

**Organization**: Tasks are grouped by user story so each story can be implemented, tested, and shipped independently. Setup, Foundational, and Polish phases carry no story label. US1–US5 phases carry `[US1]` / `[US2]` / `[US3]` / `[US4]` / `[US5]`.

## Format: `- [ ] TXXX [P?] [Story?] Description with file path`

- **[P]**: Different file, no incomplete deps — safe to run in parallel.
- **[Story]**: REQUIRED on US1–US5 phase tasks; OMITTED on Setup / Foundational / Polish.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Workspace prep — new crate registration, new deploy directories, doc placeholders. No business logic.

- [X] T001 Add `tools/bench` to `members` (and `default-members` if used) in [Cargo.toml](../../Cargo.toml); add a `[workspace.lints]`-aware `unsafe_code = "forbid"` placeholder if the new member needs an override.
- [X] T002 [P] Scaffold the `klams-bench` crate at [tools/bench/Cargo.toml](../../tools/bench/Cargo.toml) with `[package] name = "klams-bench"`, `publish = false`, two `[[bin]]` entries (`seed`, `run`), and dev-style deps `rand = "0.8"`, `rand_chacha = "0.3"`, `hdrhistogram = "7"`, plus workspace-relative `klams-store`, `klams-types`, `tokio`, `anyhow`, `serde_json`, `chrono`, `uuid`.
- [X] T003 [P] Create empty stubs [tools/bench/src/lib.rs](../../tools/bench/src/lib.rs), [tools/bench/src/bin/seed.rs](../../tools/bench/src/bin/seed.rs), [tools/bench/src/bin/run.rs](../../tools/bench/src/bin/run.rs), and [tools/bench/README.md](../../tools/bench/README.md) with header comments only so `cargo check --workspace` is green.
- [X] T004 [P] Create the `deploy/prometheus/` directory with [deploy/prometheus/README.md](../../deploy/prometheus/README.md) describing how the scrape job composes with the existing compose stack (placeholder text — real content lands in US4).
- [X] T005 [P] Add `bench-seed` and `bench-run` recipes to [justfile](../../justfile) wired to `cargo run -p klams-bench --bin seed` / `--bin run`. Recipes must always exit 0 per FR-022.
- [X] T006 Verify `cargo check --workspace` and `just gate` still pass with the new empty `klams-bench` member.

**Checkpoint**: Workspace skeleton accepts new crate; no functional changes yet.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared query layer in `klams-store`, config additions, new error codes, projection extension. These MUST land before US1, US2, or US3 implementation can start.

⚠️ **CRITICAL**: No US1/US2/US3 work begins until this phase is complete and `cargo test -p klams-store` is green.

- [X] T007 Add `ApiConfig::memories_max_window_days: u32` (default 30) to [crates/klams-types/src/config.rs](../../crates/klams-types/src/config.rs), with serde default + doc comment citing FR-009.
- [X] T008 [P] Extend the `Memory` projection in [crates/klams-types/src/lib.rs](../../crates/klams-types/src/lib.rs) (or wherever `Memory` lives) with optional `deleted_at: Option<DateTime<Utc>>` and `deleted_by_author_id: Option<Uuid>`, gated to serialize only when present (FR-010). No change for live rows.
- [X] T009 Add `ListMemoriesQuery`, `ListMemoriesRow`, `EventSearchQuery`, and the cursor helper types (matching [data-model.md](./data-model.md) §Query Layer) to [crates/klams-store/src/lib.rs](../../crates/klams-store/src/lib.rs), plus the new trait methods `Store::list_memories(...)` and `Store::event_search(...)`.
- [X] T010 Implement the composite `list_memories` + `event_search` orchestrator in [crates/klams-store/src/composite.rs](../../crates/klams-store/src/composite.rs), including the shared date-window + cursor decode helper and the kind-merge sort.
- [X] T011 [P] Implement `list_memories_facts_page`, `list_memories_events_page`, and `event_search_page` (with `payload_match` → JSONB containment) in [crates/klams-store/src/postgres.rs](../../crates/klams-store/src/postgres.rs).
- [X] T012 [P] Implement `list_memories_knowledge_page` (payload-only date-window scroll, no embedding) in [crates/klams-store/src/qdrant.rs](../../crates/klams-store/src/qdrant.rs).
- [X] T013 Add stable error codes `WINDOW_TOO_LARGE` and `INVALID_WINDOW` to [crates/klams-mcp/src/errors.rs](../../crates/klams-mcp/src/errors.rs) and add `WindowTooLarge` / `InvalidWindow` variants → 400 mappings in [crates/klams-api/src/error.rs](../../crates/klams-api/src/error.rs), referencing [contracts/error-codes.md](./contracts/error-codes.md).
- [X] T014 [P] Add cursor encode/decode parity unit test in [crates/klams-store/tests/cursor_v2.rs](../../crates/klams-store/tests/cursor_v2.rs) confirming round-trip compatibility with sprint 007's `(created_at, id)` base64 format.
- [X] T015 Add the store-level integration test [crates/klams-store/tests/store_list_memories.rs](../../crates/klams-store/tests/store_list_memories.rs) covering cross-author, multi-kind, cursor continuity, and the soft-deleted projection surfacing.

**Checkpoint**: `Store::list_memories` and `Store::event_search` callable from any crate; config + error codes available; foundational tests green.

---

## Phase 3: US1 — `event_search` MCP tool (Priority: P1) 🎯 MVP slice 1

**Goal**: Agents holding a `read`-scoped token can retrieve events filtered by `author_id`, `category`, `since`, `until`, and `payload_match`, with cursor pagination, no embedding pipeline, newest-first by default.

**Independent Test**: Append several events with distinct `category` / `payload` via `memory_append_event`. From a separate MCP client, call `event_search({category: "Deploy", since: "<1h ago>"})` and confirm only matching events return, newest-first, each carrying `author.agent_name` / `author.model`. Verify with `payload_match: {"service": "widget"}` that exact-equality JSON match works. Verify `read`-only token succeeds.

### Implementation & tests for US1

- [X] T016 [US1] Define the `event_search` tool input struct with `schemars`-derived JSON Schema in [crates/klams-mcp/src/tools/event_search.rs](../../crates/klams-mcp/src/tools/event_search.rs), matching [contracts/tool-schemas/event_search.json](./contracts/tool-schemas/event_search.json).
- [X] T017 [US1] Implement the `event_search` `#[tool]` handler in the same file: scope-gate to `read`, validate window (`since > until` → `INVALID_WINDOW`; window > `memories_max_window_days` → `WINDOW_TOO_LARGE`), call `Store::event_search`, project rows to public `Memory { kind: "event", ... }`, attach author subset.
- [X] T018 [US1] Register `event_search` in the tool table at [crates/klams-mcp/src/tools/mod.rs](../../crates/klams-mcp/src/tools/mod.rs) under the `read` scope so it surfaces in `tools/list` per FR-003.
- [X] T019 [P] [US1] Add tracing span coverage (token-hash, `agent_name`, `model`, requested window, result count) in [crates/klams-mcp/src/tools/event_search.rs](../../crates/klams-mcp/src/tools/event_search.rs); no PII, no `author_id` label.
- [X] T020 [P] [US1] Add integration test [crates/klams-mcp/tests/mcp_event_search.rs](../../crates/klams-mcp/tests/mcp_event_search.rs) covering FR-001..FR-005 — category filter, payload_match exact-equality, cursor pagination, `read`-scope success, no-embedding (assert TEI never called).
- [X] T021 [P] [US1] Add edge-case integration test [crates/klams-mcp/tests/mcp_event_search_window.rs](../../crates/klams-mcp/tests/mcp_event_search_window.rs) covering empty intersection, inverted window → `INVALID_WINDOW`, oversized window → `WINDOW_TOO_LARGE`.

**Checkpoint**: `event_search` callable end-to-end; sprint 007 acceptance tests still green.

---

## Phase 4: US3 — `GET /v1/memories` HTTP endpoint (Priority: P1) 🎯 MVP slice 2

**Goal**: Cross-author, all-kinds listing endpoint paginated newest-first, filtered by `since`/`until`/`kinds`/`state`/`authors`, gated by `read` scope, bounded by the 30-day window cap.

**Independent Test**: Seed fixture data covering all three kinds across two authors in the last day. `curl -H 'Authorization: Bearer <read-token>' '/v1/memories?since=<24h ago>'` returns the unified projection newest-first with a cursor when more pages exist. Request a 31-day window → `400 WINDOW_TOO_LARGE` with `max_window_days: 30` in the body. Request with `state=all` → soft-deleted rows include `deleted_at` and `deleted_by_author_id`. Use a token without `read` scope → `403 INSUFFICIENT_SCOPE`.

**Story ordering note**: US3 is implemented before US2 because the viewport Activity tab (US2) wraps this endpoint via a Tauri command.

### Implementation & tests for US3

- [X] T022 [US3] Create the handler [crates/klams-api/src/handlers/memories.rs](../../crates/klams-api/src/handlers/memories.rs): parse query params per FR-007, default `since = now − 24h` / `until = now`, validate window (inverted → `INVALID_WINDOW`; > `memories_max_window_days` → `WINDOW_TOO_LARGE`), call `Store::list_memories`, project to public `Memory` with soft-delete fields when `state ∈ {deleted, all}` (FR-010).
- [X] T023 [US3] Mount `GET /v1/memories` under the `read` scope in [crates/klams-api/src/router.rs](../../crates/klams-api/src/router.rs); confirm `INSUFFICIENT_SCOPE` flow matches sprint 007.
- [X] T024 [P] [US3] Add tracing span coverage (token-hash, requested window, kinds, state, authors-count, result count) in [crates/klams-api/src/handlers/memories.rs](../../crates/klams-api/src/handlers/memories.rs).
- [X] T025 [P] [US3] Add integration test [crates/klams-api/tests/api_memories_list.rs](../../crates/klams-api/tests/api_memories_list.rs) covering FR-006..FR-011 — defaults, `kinds=fact,event` filter, `state=live` default, `authors=<uuid1>,<uuid2>` filter, projection field shape, `403 INSUFFICIENT_SCOPE` without `read` scope, cursor continuity across pages.
- [X] T026 [P] [US3] Add integration test [crates/klams-api/tests/api_memories_window_cap.rs](../../crates/klams-api/tests/api_memories_window_cap.rs) covering FR-009 — 31-day window → `400 WINDOW_TOO_LARGE` with the configured maximum surfaced; `since > until` → `400 INVALID_WINDOW`.
- [X] T027 [P] [US3] Add integration test [crates/klams-api/tests/api_memories_deleted_state.rs](../../crates/klams-api/tests/api_memories_deleted_state.rs) covering FR-010 — `state=deleted` and `state=all` surface `deleted_at` + `deleted_by_author_id`; `state=live` (default) omits them.
- [X] T028 [P] [US3] Create memories fixture data under [tests/fixtures/memories/](../../tests/fixtures/memories/) covering all kinds + at least one soft-deleted fact row.

**Checkpoint**: `GET /v1/memories` responds correctly under every documented filter combination; quickstart steps for US3 pass.

---

## Phase 5: US2 — Viewport Activity tab (Priority: P1) 🎯 MVP slice 3

**Goal**: New `/activity` SvelteKit route renders a single unified cross-author, all-kinds list with kind/state/author filters and cursor-driven pagination, backed by a new Tauri command wrapping `GET /v1/memories`.

**Independent Test**: With klams running and fixture writes from two authors across all kinds, open the viewport, click "Activity" in the nav bar, and confirm: (a) every seeded row appears with correct kind badge, state, tags, author; (b) kind filter narrows to `event` only; (c) state filter `soft-deleted` shows only previously-deleted rows; (d) clicking a row navigates to the existing `/facts/:id` / `/knowledge/:id` / `/events/:id` detail route; (e) "next page" loads via cursor without resetting filters.

⚠️ **Depends on US3** (T022–T023) being complete — Activity tab cannot render without `GET /v1/memories`.

### Implementation & tests for US2

- [X] T029 [US2] Add the Tauri command in [viewport/src-tauri/src/commands/memories.rs](../../viewport/src-tauri/src/commands/memories.rs): accepts `ListMemoriesRequest`, calls `GET /v1/memories` on the configured klams-service base URL, returns the parsed response. Register the command in the Tauri builder.
- [X] T030 [P] [US2] Add TypeScript types in [viewport/src/lib/types/memories.ts](../../viewport/src/lib/types/memories.ts): `ListMemoriesRequest`, `MemoryItem`, `ListMemoriesResponse` matching [contracts/rest-memories.md](./contracts/rest-memories.md) exactly.
- [X] T031 [US2] Add the SvelteKit loader [viewport/src/routes/activity/+page.ts](../../viewport/src/routes/activity/+page.ts) that calls the Tauri command with default `since = now − 24h`, `until = now`, `state = live`, all kinds, no author filter.
- [X] T032 [US2] Add the Activity view [viewport/src/routes/activity/+page.svelte](../../viewport/src/routes/activity/+page.svelte): from/to datetime pickers, kind multi-select, state radio (live / soft-deleted / all), author multi-select (optional), cursor-driven "next page" button. Reuse per-author drilldown row primitives for kind badge / state / tags / deep-link.
- [X] T033 [P] [US2] Add the "Activity" nav entry in the viewport nav bar (search for the existing nav component near `/authors`).
- [X] T034 [P] [US2] Add integration test [viewport/src-tauri/tests/viewport_activity_command.rs](../../viewport/src-tauri/tests/viewport_activity_command.rs) for the Tauri command round-trip against a mocked klams-service per FR-014.

**Checkpoint**: Activity tab renders the seeded corpus in under 1 s (SC-002); every filter works; row click navigates correctly. MVP complete.

---

## Phase 6: US4 — Grafana panel fix + Prometheus scrape config (Priority: P2)

**Goal**: Three "MCP author activity" panels render real data in the existing Grafana dashboard after a clean compose restart; Prometheus scrape config is checked in and reproducible from a fresh clone.

**Independent Test**: From a clean checkout, run the compose stack with the new `observability` profile, drive at least one write / delete / search from a registered author, restart prometheus + grafana per the documented runbook, open the klams Grafana dashboard, confirm the three panels render non-empty series broken down by `agent_name` / `model` (and `kind` on writes, `mode` on deletes) within one scrape interval.

### Implementation & verification for US4

- [X] T035 [US4] Author the scrape job in [deploy/prometheus/prometheus.yml](../../deploy/prometheus/prometheus.yml) targeting `klams-service:9000/metrics` (or the documented port), matching [contracts/prometheus-scrape.md](./contracts/prometheus-scrape.md). Include a commented compose-mode hostname block.
- [X] T036 [US4] Update [deploy/prometheus/README.md](../../deploy/prometheus/README.md) with the real wiring instructions (replace Phase 1 placeholder).
- [X] T037 [P] [US4] Wire a `observability` compose profile in [deploy/docker-compose.yml](../../deploy/docker-compose.yml) that mounts `deploy/prometheus/prometheus.yml` and brings up the prometheus + grafana services.
- [X] T038 [US4] Add the three "MCP author activity" panels to [deploy/grafana/klams.json](../../deploy/grafana/klams.json): writes panel (`rate(klams_mcp_writes_total[5m]) by (agent_name, model, kind)`), deletes panel (`rate(klams_mcp_deletes_total[5m]) by (agent_name, model, mode)`), search panel (`rate(klams_mcp_search_total[5m]) by (agent_name, model)`). PromQL must match [contracts/grafana-mcp-panels.md](./contracts/grafana-mcp-panels.md) and the labels actually emitted by [crates/klams-mcp/src/metrics.rs](../../crates/klams-mcp/src/metrics.rs).
- [X] T039 [US4] Manual quickstart verification per [quickstart.md](./quickstart.md) US4 steps — record the result in the sprint quickstart walk-through.

**Checkpoint**: Sprint 007 SC-005 is finally verifiable; panels render real data from a clean checkout (FR-016, FR-017, FR-018).

---

## Phase 7: US5 — Perf baseline (Priority: P2)

**Goal**: Deterministic seeded fixture + 100-call `memory_search` harness producing a checked-in `perf-baseline.md` with p50/p95/p99, linked from the top-level README. Harness always exits 0 (FR-022).

**Independent Test**: Run `just bench-seed` against a fresh test DB + Qdrant; confirm ≥ 10k facts and ≥ 50k knowledge items exist. Run `just bench-run` and confirm [specs/008-activity-observability/perf-baseline.md](./perf-baseline.md) is (re)written with a markdown table containing p50 / p95 / p99 in ms. Run `bench-seed` twice with the same seed and confirm equivalent corpora. Confirm a link to `perf-baseline.md` is in the top-level README.

### Implementation & tests for US5

- [X] T040 [US5] Implement the deterministic corpus generator in [tools/bench/src/lib.rs](../../tools/bench/src/lib.rs) using `ChaCha20Rng::seed_from_u64(...)`; expose `generate_facts(n)` and `generate_knowledge(n)` returning identical sequences for identical seeds (FR-019).
- [X] T041 [P] [US5] Implement [tools/bench/src/bin/seed.rs](../../tools/bench/src/bin/seed.rs): CLI parses `--seed`, `--facts`, `--knowledge`, `--klams-url`, `--token`; defaults 10_000 / 50_000; writes via the existing klams write surfaces; logs progress every N rows.
- [X] T042 [P] [US5] Implement the histogram + markdown serializer in [tools/bench/src/lib.rs](../../tools/bench/src/lib.rs) using `hdrhistogram::Histogram::<u64>` (microseconds, 3 sig figs) → markdown table with p50 / p95 / p99 in ms.
- [X] T043 [US5] Implement [tools/bench/src/bin/run.rs](../../tools/bench/src/bin/run.rs): runs `memory_search` 100× with a representative query, records latencies into the histogram, writes [specs/008-activity-observability/perf-baseline.md](./perf-baseline.md). Per FR-022, always exits 0 regardless of measured numbers; no comparison to SC-006 threshold (FR-022).
- [X] T044 [P] [US5] Add fixture-determinism unit test [tools/bench/tests/fixture_determinism.rs](../../tools/bench/tests/fixture_determinism.rs) asserting same seed → byte-identical corpus.
- [X] T045 [P] [US5] Generate the initial [specs/008-activity-observability/perf-baseline.md](./perf-baseline.md) by running `just bench-seed && just bench-run` against the test compose stack; commit the resulting markdown. Smoke-sized (500 / 2000) — full corpus rerun tracked in [specs/planning/backlog.md](../planning/backlog.md) pending klams#26.
- [X] T046 [US5] Update [README.md](../../README.md) with a one-line link: `[Performance baseline](specs/008-activity-observability/perf-baseline.md)`.
- [X] T047 [P] [US5] Flesh out [tools/bench/README.md](../../tools/bench/README.md) with operator-facing usage notes, flag reference, and the "harness never gates `just gate`" guarantee.

**Checkpoint**: Sprint 007 SC-006 is measurable; perf-baseline.md committed; README link visible.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, gate verification, quickstart walkthrough, tasks self-check. No new functionality.

- [X] T048 [P] Update [docs/architecture.md](../../docs/architecture.md) §2f: shared query layer (`Store::list_memories` / `Store::event_search`), two-surfaces-one-query rationale (R-001), and the panel fix narrative.
- [X] T049 [P] Update [docs/setup.md](../../docs/setup.md) with the new `deploy/prometheus/` wiring, the `observability` compose profile, and the Grafana reload note.
- [X] T050 [P] Update [docs/usage.md](../../docs/usage.md): add an "Activity tab" section and a new `event_search` row in the MCP tool table.
- [X] T051 Run `just gate` from repo root and resolve any fmt / clippy / test failures.
- [X] T052 Walk through every step of [quickstart.md](./quickstart.md) against a running stack; confirm all 10 steps pass; record any drift back into spec/plan as needed. Steps 1–9 validated end-to-end against the live `kubs0` stack during Phases 1–7; step 10 (full-corpus perf rerun) is gated on kwi work item #26 and tracked in [specs/planning/backlog.md](../planning/backlog.md). The committed [perf-baseline.md](./perf-baseline.md) is auto-tagged as a smoke run.
- [X] T053 Self-check this [tasks.md](./tasks.md): every task uses `- [ ] TXXX [P?] [Story?] Description with file path`; Setup/Foundational/Polish carry no story label; US1–US5 phases carry the correct label; counts match the summary below.
- [X] T054 [P] Acceptance test: clicking a soft-deleted row in the viewport Activity tab navigates to the per-kind detail route and that detail view renders the soft-deleted state (`state=deleted`, `deleted_at`, `deleted_by`) without error. Verifies FR-015a. File: [viewport/src/routes/activity/row.test.ts](../../viewport/src/routes/activity/row.test.ts) (vitest — chosen over Playwright/Tauri-harness because no DOM/e2e tooling is configured in the viewport project; the row-helper module is the unit of behaviour for FR-015a and is wired into `+page.svelte`).

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies; can start immediately.
- **Foundational (Phase 2)**: depends on Setup; BLOCKS US1, US2, US3.
- **US1 (Phase 3)**: depends on Foundational. Independent of US2–US5.
- **US3 (Phase 4)**: depends on Foundational. Independent of US1, US4, US5. **Blocks US2.**
- **US2 (Phase 5)**: depends on Foundational + US3 (Tauri command wraps `GET /v1/memories`).
- **US4 (Phase 6)**: depends only on Setup (Phase 1 created `deploy/prometheus/`). Independent of US1, US2, US3, US5.
- **US5 (Phase 7)**: depends on Setup (Phase 1 scaffolded `klams-bench`) and on existing `memory_search`; independent of US1–US4.
- **Polish (Phase 8)**: depends on every story being complete that you intend to ship.

### Within-Phase Dependencies

- T009 (trait + types) blocks T010, T011, T012, T015.
- T013 (error codes) blocks T017 (US1 handler), T022 (US3 handler).
- T022 (US3 handler) blocks T029 (US2 Tauri command).
- T040 (corpus generator) blocks T041 (seed bin), T042 (serializer) blocks T043 (run bin), T043 blocks T045 (initial baseline).

### Parallel Opportunities

- **Within Phase 1**: T002 ∥ T003 ∥ T004 ∥ T005 (different files, independent).
- **Within Phase 2 (after T009)**: T011 ∥ T012 ∥ T014 (postgres, qdrant, cursor test — all distinct files).
- **Within Phase 3 (after T017)**: T019 ∥ T020 ∥ T021 (tracing addition vs two test files).
- **Within Phase 4 (after T022, T023)**: T024 ∥ T025 ∥ T026 ∥ T027 ∥ T028 (tracing + four independent test/fixture files).
- **Within Phase 5 (after T029, T031)**: T030 ∥ T033 ∥ T034 (types, nav entry, Tauri test).
- **Within Phase 6 (after T035)**: T037 (compose profile) and T038 (Grafana panels) touch different files.
- **Within Phase 7 (after T040)**: T041 ∥ T042 ∥ T044 ∥ T047 (seed bin, serializer, determinism test, README).
- **Phase 8**: T048 ∥ T049 ∥ T050 (three distinct docs).
- **Cross-phase**: once Foundational closes (T015), US1 (Phase 3), US3 (Phase 4), US4 (Phase 6), US5 (Phase 7) can all proceed in parallel; US2 waits on US3.

---

## Parallel Example: User Story 1 (after T018)

```bash
# Three independent files; safe to run together:
Task: "Add tracing span coverage in crates/klams-mcp/src/tools/event_search.rs"          # T019
Task: "Integration test crates/klams-mcp/tests/mcp_event_search.rs"                       # T020
Task: "Edge-case test crates/klams-mcp/tests/mcp_event_search_window.rs"                  # T021
```

## Parallel Example: User Story 3 (after T022, T023)

```bash
Task: "Tracing spans in crates/klams-api/src/handlers/memories.rs"                        # T024
Task: "Integration test crates/klams-api/tests/api_memories_list.rs"                      # T025
Task: "Window-cap test crates/klams-api/tests/api_memories_window_cap.rs"                 # T026
Task: "Deleted-state test crates/klams-api/tests/api_memories_deleted_state.rs"           # T027
Task: "Fixtures under tests/fixtures/memories/"                                            # T028
```

---

## Implementation Strategy

### MVP scope

The MVP for this sprint is the **operator + agent retrieval triangle**: US1 (`event_search`), US3 (`GET /v1/memories`), and US2 (Activity tab). All three are P1. They become demoable together once Phases 1–5 are complete.

1. Complete Phase 1 (Setup).
2. Complete Phase 2 (Foundational) — this is the only hard sequencing gate.
3. In parallel (or sequentially, by single contributor):
   - Phase 3 (US1) → `event_search` agent-ready.
   - Phase 4 (US3) → `GET /v1/memories` HTTP-ready.
4. Phase 5 (US2) — viewport Activity tab. Requires Phase 4 first.
5. **STOP & VALIDATE**: walk steps 1–7 of [quickstart.md](./quickstart.md). Deploy/demo if green.

### Incremental delivery (P2 phases)

6. Phase 6 (US4) — Grafana panel fix. Verifies sprint 007 SC-005 retrospectively; pure config change.
7. Phase 7 (US5) — perf baseline. Generates the artifact that unblocks every future "did my change regress search?" question; per FR-022 never gates `just gate`.
8. Phase 8 (Polish) — docs, gate pass, full quickstart walk-through.

### Parallel team strategy

After Phase 2 closes:

- Dev A: Phase 3 (US1).
- Dev B: Phase 4 (US3) → Phase 5 (US2).
- Dev C: Phase 6 (US4) — completely orthogonal, can start as soon as Setup is done.
- Dev D: Phase 7 (US5) — depends only on Setup + existing `memory_search`.

---

## Summary

- **Total tasks**: 54
- **Per phase**:
  - Phase 1 (Setup): 6 — T001–T006
  - Phase 2 (Foundational): 9 — T007–T015
  - Phase 3 (US1 — P1, `event_search`): 6 — T016–T021
  - Phase 4 (US3 — P1, `GET /v1/memories`): 7 — T022–T028
  - Phase 5 (US2 — P1, Activity tab): 6 — T029–T034
  - Phase 6 (US4 — P2, Grafana fix): 5 — T035–T039
  - Phase 7 (US5 — P2, perf baseline): 8 — T040–T047
  - Phase 8 (Polish): 7 — T048–T054
- **Per user story** (priority shown):
  - US1 (P1): 6 tasks
  - US2 (P1): 6 tasks
  - US3 (P1): 7 tasks
  - US4 (P2): 5 tasks
  - US5 (P2): 8 tasks
- **Parallel opportunities**: documented per-phase above; the largest fan-out is the Foundational → US1/US3/US4/US5 cross-phase parallelism after T015.
- **Independent test criteria**: spelled out at the top of each US phase; each story is independently demoable.
- **Format check**: every task uses `- [ ] TXXX [P?] [Story?] Description with file path`. Setup/Foundational/Polish carry no story label. US1–US5 phase tasks carry the correct `[USn]` label. T053 re-verifies this at sprint end.
- **MVP suggestion**: US1 + US3 + US2 (in that order, since US2 depends on US3) — the three P1 stories shipped together as the operator+agent retrieval triangle. US4 and US5 follow as P2 increments.
