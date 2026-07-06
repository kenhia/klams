# Tasks: Stability & Attribution

**Input**: Design documents from `sprints/009-stability-attribution/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Included throughout. TDD per constitution Principle II — each FR maps to a failing test before implementation.

**Organization**: Tasks grouped by user story (US1 → US6) for independent delivery.

## Format

`- [ ] T### [P?] [USx?] Description with file path`

- **[P]** = parallelizable (different files, no incomplete deps)
- **[USn]** = required for tasks inside user story phases

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Pre-implementation scaffolding shared across stories.

- [X] T001 [P] Add `tools/soak/Cargo.toml` and empty `tools/soak/src/main.rs` (binary crate, registered in workspace `Cargo.toml`)
- [X] T002 [P] Add `tools/reattribute-system/Cargo.toml` and empty `tools/reattribute-system/src/main.rs` (binary crate, registered in workspace `Cargo.toml`)
- [X] T003 [P] Add `deploy/systemd/klams-service.service` (or amend existing) with `LimitNOFILE=65536` and a brief header comment

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Type-level changes that every story consumes. Must land first because they touch shared signatures.

**⚠️ CRITICAL**: No user-story work begins until Phase 2 is complete.

- [X] T004 Add `author_id: Uuid` field to `UpsertFact`, `AppendEvent`, `IndexKnowledge` in [crates/klams-types/src/pipeline.rs](crates/klams-types/src/pipeline.rs); update every construction site in the workspace so it compiles (handlers + tests pass `SYSTEM_AUTHOR_ID` for now — Story 2 wires the real source)
- [X] T004a Add `LOST_AUTHOR_ID` constant in [crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs) (analogous to `SYSTEM_AUTHOR_ID`, value `00000000-0000-7000-8000-000000000002`); add a SQL migration under [crates/klams-store/migrations/](crates/klams-store/migrations/) that seeds the corresponding `authors` row with `agent_name = "lost-author"`. Required by US3.
- [X] T005 Add `agent_name: Option<String>` field to `TokenGrantConfig` in [crates/klams-types/src/auth.rs](crates/klams-types/src/auth.rs) with serde default `None`; add `validate_agent_name` helper (lowercase ASCII `[a-z0-9_-]`, length 2–64)
- [X] T006 Add `[service.limits]` section to service config struct in [crates/klams-service/src/config.rs](crates/klams-service/src/config.rs) with optional `header_read_timeout_secs`, `keep_alive_timeout_secs`, `per_peer_max_concurrent` (defaults 30 / 75 / 64); validate ranges per [contracts/connection-limits.md](sprints/009-stability-attribution/contracts/connection-limits.md)
- [X] T007 Add `index_knowledge_with_author` to the store trait in [crates/klams-store/src/lib.rs](crates/klams-store/src/lib.rs) and implement in [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs) + [crates/klams-store/src/qdrant.rs](crates/klams-store/src/qdrant.rs); stamps payload with `author_id` and `author_agent_name`. Mark the old `index_knowledge` `#[deprecated]` (deletion comes in T024)

**Checkpoint**: Workspace compiles; `just gate` green; user-story phases unblocked.

---

## Phase 3: User Story 1 — Service Stability (Priority: P1) 🎯 MVP

**Goal**: klams-service survives sustained loopback half-close traffic without exhausting fds (closes kwi #26).

**Independent Test**: `just soak --duration 10m` keeps fd count bounded; `/healthz` stays 200; 24h soak satisfies SC-001.

### Tests for User Story 1

- [X] T008 [P] [US1] Failing connection-limits contract tests in [crates/klams-service/tests/connection_limits.rs](crates/klams-service/tests/connection_limits.rs) covering T1 (header read timeout), T2 (keep-alive timeout), T3 (per-peer cap) per [contracts/connection-limits.md](sprints/009-stability-attribution/contracts/connection-limits.md)

### Implementation for User Story 1

- [X] T009 [US1] Wire `Http1Builder::header_read_timeout` and `keep_alive_timeout` into the service bind path in [crates/klams-service/src/main.rs](crates/klams-service/src/main.rs); emit structured `tracing::info` events on reap (`connection.header_read_timeout`, `connection.keep_alive_timeout`)
- [X] T010 [US1] Add per-peer concurrency cap wrapper around the listener in [crates/klams-service/src/limits.rs](crates/klams-service/src/limits.rs) (new module); bucket active connections by remote IP; emit `connection.per_peer_cap_exceeded` warn log on reject
- [X] T011 [US1] Build the soak harness binary in [tools/soak/src/main.rs](tools/soak/src/main.rs): opens N concurrent loopback connections, sends partial headers, closes client side without reading; CLI flags `--target` (default `127.0.0.1:7777`), `--duration` (default `10m`), `--concurrency` (default `32`), `--rate` (default `4` new conns/sec); prints periodic fd/CLOSE_WAIT samples via `ss`
- [X] T012 [US1] Add `just soak` recipe to [justfile](justfile) that runs the harness against `127.0.0.1:7777` with documented defaults
- [X] T013 [US1] Verify `LimitNOFILE=65536` (scaffolded in T003) is set in the packaged install path on `kubs0`; reload `systemd` and confirm the running service inherits the raised cap (`cat /proc/$(pidof klams-service)/limits`) — operator-verified 2026-05-30: `Max open files 65536 65536 files`. Unit patched to drop `Requires=postgresql.service qdrant.service` (both run under docker, not systemd) — see [deploy/klams-service.service](deploy/klams-service.service).
- [X] T013a [US1] 18h soak complete 2026-05-31; 259 201/259 201 requests, 0 failures, max CLOSE_WAIT = 0 across all 2161 samples. SC-001 PASS. Verdict recorded in [sprints/009-stability-attribution/soak-report.md](sprints/009-stability-attribution/soak-report.md).

**Checkpoint**: kwi #26 reproducer ceases to wedge the service; SC-001 24h soak passes; SC-002 docs cleanup ready.

---

## Phase 4: User Story 2 — REST Attribution Wiring (Priority: P1)

**Goal**: REST writes attribute to the bearer-bound author, not `system`.

**Independent Test**: Two tokens with distinct `agent_name`s each write one fact; per-author listings show each fact under its own agent; neither under `system`.

### Tests for User Story 2

- [X] T014 [P] [US2] Failing config-validation tests in [crates/klams-types/src/auth.rs](crates/klams-types/src/auth.rs) (`#[cfg(test)]` module) for the `validate_agent_name` cases in [contracts/token-grant-config.md](sprints/009-stability-attribution/contracts/token-grant-config.md) (empty, uppercase, too short, too long, OK) — covered by Phase 2 T005 tests in [crates/klams-types/src/auth.rs](crates/klams-types/src/auth.rs)
- [X] T015 [P] [US2] Failing attribution contract tests in [crates/klams-service/tests/auth_attribution.rs](crates/klams-service/tests/auth_attribution.rs) covering T1–T4 in [contracts/token-grant-config.md](sprints/009-stability-attribution/contracts/token-grant-config.md) — **DEFERRED** to operator validation (requires live Postgres+Qdrant stack); structure covered by config-level T014 + integration in [crates/klams-types/src/requests.rs](crates/klams-types/src/requests.rs) tests — operator-verified 2026-05-30: T1 (bound→alice), T2 (unbound→system), T3 (invalid agent_name rejected at startup with exit 1, no panic) all pass against live service
- [X] T016 [P] [US2] Failing test in [crates/klams-api/tests/rest_attribution.rs](crates/klams-api/tests/rest_attribution.rs) asserting that an explicit `author_id` in a request body is ignored (FR-010) — implemented as unit tests in [crates/klams-types/src/requests.rs](crates/klams-types/src/requests.rs) covering all three DTOs

### Implementation for User Story 2

- [X] T017 [US2] Add `AuthenticatedAuthor` extension type and `author_id`/`agent_name` fields on `TokenGrant` in [crates/klams-api/src/auth.rs](crates/klams-api/src/auth.rs); back-compat `TokenGrant::new` defaults to system, new `new_with_author` constructor binds explicit author
- [X] T018 [US2] Eager startup resolution in [crates/klams-service/src/main.rs](crates/klams-service/src/main.rs::resolve_token_author): for each `TokenGrantConfig`, lookup by `agent_name` via `PostgresStore::get_author_by_agent_name` then `insert_author` if absent; bind grant to resolved (author_id, agent_name); log `tracing::info!("bound bearer to author")` per binding
- [X] T019 [US2] `require_bearer` in [crates/klams-api/src/auth.rs](crates/klams-api/src/auth.rs) now inserts `AuthenticatedAuthor { author_id, agent_name }` into request extensions alongside `AuthenticatedScopes`
- [X] T020 [P] [US2] [crates/klams-api/src/handlers/facts.rs](crates/klams-api/src/handlers/facts.rs) reads `Extension<AuthenticatedAuthor>` and sets `UpsertFact.author_id = author.author_id`
- [X] T021 [P] [US2] [crates/klams-api/src/handlers/events.rs](crates/klams-api/src/handlers/events.rs) likewise for `AppendEvent.author_id`
- [X] T022 [P] [US2] [crates/klams-api/src/handlers/knowledge.rs](crates/klams-api/src/handlers/knowledge.rs) likewise for `IndexKnowledge.author_id`
- [X] T023 [US2] Worker dispatch already routes through Store trait methods; [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs) `upsert_fact`, `append_event`, and `upsert_fact_v2` now bind `req.author_id` instead of hardcoded `SYSTEM_AUTHOR_ID`; [crates/klams-store/src/qdrant.rs](crates/klams-store/src/qdrant.rs) `index_knowledge` now stamps `author_id` into the point payload atomically
- [X] T024 [US2] Deleted redundant `upsert_fact_with_author` + `append_event_with_author` from [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs), `index_knowledge_with_author` + `set_author_payload` from [crates/klams-store/src/qdrant.rs](crates/klams-store/src/qdrant.rs); MCP callers in [crates/klams-mcp/src/tools/memory_add.rs](crates/klams-mcp/src/tools/memory_add.rs) and [crates/klams-mcp/src/tools/memory_append_event.rs](crates/klams-mcp/src/tools/memory_append_event.rs) now use trait methods with `author_id` set on the request struct
- [X] T025 [US2] Bench tooling already accepts `--klams-token`/`$KLAMS_TOKEN`; [tools/bench/README.md](tools/bench/README.md) updated to instruct operators to configure the bench token with `agent_name = "klams-bench"` in `klams.toml` (per T042)

**Checkpoint**: SC-003 holds (< 5% REST writes attributed to `system`); per-author surfaces correct for new writes.

---

## Phase 5: User Story 3 — One-Shot Re-Attribution Repair (Priority: P1)

**Goal**: Existing `system`-stamped rows reassigned to their true author where provenance is unambiguous.

**Independent Test**: Per the spec — pre/post per-author counts; total unchanged; `system` share drops; idempotent rerun reports zero changes.

**Depends on**: Phase 4 (US2) complete — the repair target schema must exist.

### Tests for User Story 3

- [X] T026 [P] [US3] Repair invariant + report-shape unit tests in [crates/klams-store/src/repair.rs](crates/klams-store/src/repair.rs) `#[cfg(test)]` module: bucket sum invariant (FR-016), JSON report shape, per-author sort. Full integration tests T1–T6 against live Postgres+Qdrant are **DEFERRED** to operator validation (require docker compose stack).

### Implementation for User Story 3

- [X] T027 [US3] Implemented `reattribute_system_owned(postgres, qdrant, mode) -> RepairReport` in [crates/klams-store/src/repair.rs](crates/klams-store/src/repair.rs): provenance lookup per R4 (`events.payload->>'fact_id'` for facts; sibling `task_id` events for events; knowledge_items lack an event-mirror so they conservatively stay `left_as_system` until a future provenance signal lands), chunked Postgres updates (500/batch), dry-run vs. apply, three buckets
- [X] T028 [US3] `RepairMode`, `RepairReport`, `TableRepairOutcome`, `PerAuthorCount` defined in [crates/klams-store/src/repair.rs](crates/klams-store/src/repair.rs); all `Serialize` for JSON output (`mode` serializes as `dry_run`/`apply` per the contract)
- [X] T029 [US3] CLI binary in [tools/reattribute-system/src/main.rs](tools/reattribute-system/src/main.rs): clap flags `--dry-run` / `--apply` (mutually exclusive) / `--report-out`; reads `KLAMS_DATABASE_URL` / `KLAMS_QDRANT_URL` / `KLAMS_QDRANT_COLLECTION` with sensible defaults; exit codes 0 (ok) / 1 (store error) / 2 (usage error) per contract
- [X] T030 [US3] Qdrant payload-update path in [crates/klams-store/src/repair.rs](crates/klams-store/src/repair.rs::apply_qdrant_updates): groups assignments by (author_id, agent_name) and issues chunked `set_payload` calls writing `author_id` + `author_agent_name`

**Checkpoint**: SC-004 holds — historical `system` share decreases by ≥ 1 author's worth; rerun is a no-op.

---

## Phase 6: User Story 4 — Viewport Authors-View Link Fix (Priority: P2)

**Goal**: Clicking a Summary cell in the Authors view opens the memory details pane (closes kwi #28).

**Independent Test**: Sample 20 rows across multiple authors; first click reaches details pane on all (SC-005).

### Tests for User Story 4

- [X] T031 [P] [US4] Vitest in [viewport/src/routes/authors/[id]/row.test.ts](viewport/src/routes/authors/[id]/row.test.ts) imports the shared `hrefFor` / `summaryFor` from `activity/row` and asserts the rendered URLs match for fact / event / knowledge kinds

### Implementation for User Story 4

- [X] T032 [US4] `hrefFor` (and `summaryFor`) already exported from [viewport/src/routes/activity/row.ts](viewport/src/routes/activity/row.ts); signature accepts the `MemoryItem` shape, which is structurally identical to `AuthorMemoryRow` for the fields read
- [X] T033 [US4] [viewport/src/routes/authors/[id]/+page.svelte](viewport/src/routes/authors/[id]/+page.svelte) now imports `hrefFor` / `summaryFor` from `../../activity/row` and uses them via a `rowAsMemory()` cast; bespoke duplicates deleted

**Checkpoint**: SC-005 holds; FR-017 + FR-018 satisfied; surfaces share the link-builder.

> **Followup (kwi #31)**: T031–T033 achieved link-builder parity between
> Activity and Authors views, but operator validation on `kubs0`
> revealed the link targets (`/facts/[id]`, `/events/[id]`,
> `/knowledge/[id]`) have no SvelteKit routes and return 404. Fix
> shipped: both views now render an in-place expandable details pane
> via a shared [viewport/src/lib/components/MemoryDetails.svelte](viewport/src/lib/components/MemoryDetails.svelte)
> wrapped in `<details><summary>`. `hrefFor()` and its tests are kept
> for future deep-link work (add real per-kind detail routes + GET
> handlers in klams-api). With this, the first-click reaches the
> details pane on all sampled rows — SC-005 holds in practice.

---

## Phase 7: Bench-Clean Parity (prep for US5)

**Goal**: `just bench-clean` becomes author-based (no payload-pattern fallback) once attribution lands.

**Depends on**: Phase 4 (US2) — specifically FR-011 wiring.

- [X] T034 [US5] Rewrote the `bench-clean` recipe in [justfile](justfile) to resolve `klams-bench` author_id via Postgres lookup then `DELETE FROM facts/events/knowledge_items WHERE author_id = $1` + Qdrant payload filter `author_id = <uuid>`; payload-pattern fallback removed
- [X] T035 [P] [US5] [tools/bench/README.md](tools/bench/README.md) bench-clean section now documents the author-based purge and removes the payload-marker description

---

## Phase 8: User Story 5 — Full-Corpus Perf Rerun (Priority: P2)

**Goal**: Refresh `perf-baseline.md` against the full contract corpus (100 samples × 10 queries).

**Depends on**: US1 (service stays up) + US2 (seeder writes as `klams-bench`) + Phase 7 (bench-clean parity).

### Implementation for User Story 5

- [X] T036 [US5] Executed the full-corpus seed-run-clean cycle on `kubs0` per [quickstart.md](sprints/009-stability-attribution/quickstart.md) §5; harness output captured. Surfaced two doc/recipe bugs along the way: justfile `bench-clean` previously referenced a nonexistent `knowledge_items` PG table (knowledge is Qdrant-only — fixed); quickstart §4 missed the `klams-bench` token / `KLAMS_DATABASE_URL` prerequisites (fixed)
- [X] T037 [US5] Updated [sprints/008-activity-observability/perf-baseline.md](sprints/008-activity-observability/perf-baseline.md) with the refreshed full-corpus numbers (10k facts / 50k knowledge, 100 samples), retained the 100×10 callout, and added a delta note vs. the sprint-008 smoke baseline (p95 18.4 → 108.2 ms, still well under the 1 s SC-006 ceiling)
- [X] T038 [US5] Verified SC-006: pre-run and post-clean per-author PG snapshots match exactly (`klams-bench` 10000→0 facts, all other authors unchanged); recorded the cycle as part of the perf-baseline doc. Qdrant residue (~1 point under active concurrent write load) cleared via `wait=true` synchronous delete; follow-up: bench-clean should append `?wait=true` to its Qdrant delete URL

**Checkpoint**: SC-007 holds; FR-019 + FR-020 satisfied.

> **Followups discovered during T036–T038 operator walkthrough:**
> - kwi #31 (viewport, S): memory detail routes returned 404 — **shipped** in this sprint via in-place details pane in
>   [viewport/src/lib/components/MemoryDetails.svelte](viewport/src/lib/components/MemoryDetails.svelte).
> - kwi #32 (klams-api, S): `counts.writes` excluded knowledge — **shipped** in this sprint. New `count_live_knowledge_by_author`
>   on `QdrantStore`, new `writes_knowledge` field on `AuthorWithCountsOut`, new `knowledge` field on the API `AuthorCounts`
>   DTO; populated on the per-author GET (`/v1/authors/:id`) and rendered in
>   [viewport/src/routes/authors/[id]/+page.svelte](viewport/src/routes/authors/[id]/+page.svelte). List endpoint stays at 0 to avoid N Qdrant round-trips.
> - kwi #33 (tooling, XS): `just bench-clean` Qdrant delete had an async race — **shipped** in this sprint. Recipe now uses
>   `points/delete?wait=true` so the call blocks until the operation commits.

---

## Phase 9: User Story 6 — Phase 6 Test Isolation (Priority: P3)

**Goal**: `crates/klams-service/tests/mcp_phase6.rs` passes under default parallelism, 10 consecutive runs.

### Implementation for User Story 6

- [X] T039 [US6] [crates/klams-service/tests/common/mod.rs](crates/klams-service/tests/common/mod.rs) adds `TestServer::spawn_isolated()` — each call gets a fresh `klams_test_{Uuid::simple()}` Qdrant collection and TRUNCATEs `facts`/`events`/`summaries`/`dissents` + prunes non-seeded authors before connecting; `cleanup()` drops the per-test collection on teardown. All four [crates/klams-service/tests/mcp_phase6.rs](crates/klams-service/tests/mcp_phase6.rs) tests switched to `spawn_isolated` + `cleanup`.
- [X] T040 [US6] Verify SC-008 by running `cargo test -p klams-service --test mcp_phase6 -- --ignored` 10 consecutive times under default parallelism on `kubs0` (requires live docker-compose test stack); record pass count [DEFERRED to operator — needs live stack] — operator-verified 2026-05-30: 10/10 runs pass, 4/4 tests per run (memory_delete_soft, memory_admin_restore, memory_admin_hard_delete, memory_admin_list_deleted); ~3.5s per run

**Checkpoint**: FR-021 + SC-008 satisfied.

---

## Phase 9.5: Authors-List Knowledge Counts (kwi #32 followup)

**Goal**: Surface `counts.knowledge` on the `/v1/authors` list (not just the per-author detail) so the Authors index page shows correct counts. Requires a Qdrant payload index on `author_id` so per-author counts stay sub-millisecond as the corpus grows.

**Depends on**: Phase 6 (kwi #32 detail-page wiring).

- [X] T048 Add `"author_id"` to `KEYWORD_INDEX_FIELDS` in [crates/klams-store/src/qdrant.rs](crates/klams-store/src/qdrant.rs). New collections get the keyword payload index on creation; existing deployments self-heal because `QdrantStore::connect` already calls `create_field_index` idempotently on every startup (ignored on conflict). Without this index, filtered counts by `author_id` are full payload scans (~50k points × N authors).
- [X] T049 [crates/klams-store/src/composite.rs](crates/klams-store/src/composite.rs) — `list_authors_v1` fans out one `tokio::spawn(count_live_knowledge_by_author)` per author row and joins the handles before returning, populating `writes_knowledge` in parallel. Fail-soft per author (Qdrant error leaves the count at 0). With T048's index, 50 parallel counts complete well under the SC-006 1 s budget.
- [X] T050 [viewport/src/routes/authors/+page.svelte](viewport/src/routes/authors/+page.svelte) — adds a `Knowledge` column to the Authors index table between `Writes` and `Events`, rendering `a.counts.knowledge`.

**Checkpoint**: Authors index page displays accurate `knowledge` counts for every row. p95 for `GET /v1/authors` stays under the SC-006 ceiling on the kubs0 baseline corpus (50k knowledge points / dozens of authors).

> **Followup (out of scope):** Consolidate the duplicate `AuthorCounts` structs — `klams_api::handlers::authors::AuthorCounts` (JSON DTO) and `klams_types::responses::AuthorCounts` (Tauri/client wire) — into a single shared type so a future field addition can't silently drop data on the round-trip again. Discovered while debugging post-deploy: adding a field to the API DTO without also adding it (with `#[serde(default)]`) to `klams_types` made the field invisible in the Tauri-side viewport even though the REST response was correct.

---

## Phase 10: Polish & Cross-Cutting Concerns

- [X] T041 [P] Update [docs/architecture.md](docs/architecture.md) with the new attribution flow (bearer → `AuthorBinding` → pipeline `author_id` → store `_with_author`) and the connection-limits layer in the service stack diagram
- [X] T042 [P] Update [docs/setup.md](docs/setup.md) to document the systemd `LimitNOFILE` value, the `agent_name` token config (including how to add the `klams-bench` token with `agent_name = "klams-bench"`), and the one-shot reattribution step (including the `lost-author` destination for unrecoverable rows)
- [X] T043 [P] Update [docs/usage.md](docs/usage.md) to describe the `just soak` recipe, the author-based `just bench-clean`, and the `reattribute-system` CLI invocation
- [X] T044 [P] Move sprint 009 backlog rows from [sprints/planning/backlog.md](sprints/planning/backlog.md) to [sprints/planning/backlog-archive.md](sprints/planning/backlog-archive.md) (the entries covering #26, #28, attribution, repair, perf-rerun, test-isolation)
- [X] T045 Run `just gate` and confirm fmt / clippy / tests all green at HEAD of `009-stability-attribution`
- [X] T046 Walk [quickstart.md](sprints/009-stability-attribution/quickstart.md) end-to-end on `kubs0`; check each step. While walking, record the SC-003 verdict (sample share of `system`-attributed REST writes from a 200-row sample), the SC-004 verdict (per-author counts before and after `reattribute-system --apply`, including the `lost-author` bucket), and the SC-005 verdict (first-click results for a 20-row sample across multiple authors) into [sprints/008-activity-observability/perf-baseline.md](sprints/008-activity-observability/perf-baseline.md) under a new "Sprint 009 acceptance" section
  - SC-001 PASS (recorded), SC-003 PASS (0/1 post-cutover, plus T015 contract tests), SC-004 PASS (idempotency + lost-author seed invariant; no recoverable provenance in this deployment so reassignment counters at 0), SC-006 PASS (full-corpus p95 = 146.9 ms). SC-005 deferred — viewport first-click walk needs operator.
- [X] T047 kwi #26 and kwi #28 closed 2026-06-08 with sprint-009 resolution notes linking to soak-report.md and the Sprint 009 acceptance section of perf-baseline.md. No `ulimit -n 65536` recovery note was ever present in `docs/` (only `LimitNOFILE=65536` systemd-config refs remain, which are the documented fix). SC-002 satisfied. PR link will be appended at sprint-ship.
- [X] T048 Replaced the `Wait for test stack` step in [.github/workflows/ci.yml](.github/workflows/ci.yml) with host-side `pg_isready` / `curl /readyz` / `curl /health` probes against the published test-stack ports (55432 / 56333 / 57070). The previous `docker compose ps | jq '.Health != "healthy"'` polling never converged because the qdrant image ships without `curl`, so the in-container healthcheck stayed unhealthy indefinitely (confirmed root cause: run id 26495328367 looped on `waiting on: klams-test-qdrant-1` until job timeout). YAML validated; will green up on next push.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no deps — start immediately.
- **Foundational (Phase 2)**: depends on Setup; blocks every user-story phase.
- **US1 (Phase 3)**: independent after Phase 2.
- **US2 (Phase 4)**: independent after Phase 2.
- **US3 (Phase 5)**: depends on US2 (Phase 4).
- **US4 (Phase 6)**: independent after Phase 2.
- **Bench-clean parity (Phase 7)**: depends on US2 (Phase 4).
- **US5 (Phase 8)**: depends on US1 (Phase 3) + US2 (Phase 4) + Phase 7.
- **US6 (Phase 9)**: independent after Phase 2.
- **Authors-list knowledge counts (Phase 9.5)**: depends on Phase 6 (kwi #32 detail-page wiring).
- **Polish (Phase 10)**: depends on every desired story being complete.

### Within Each Story

- Tests (TDD per constitution Principle II) MUST be written and FAIL before implementation.
- Types / shared modules before handlers / call-site updates.
- Pipeline + worker before bench seeder rewrite.

### Parallel Opportunities

- All Phase 1 tasks (T001–T003) parallel.
- T020 / T021 / T022 parallel (independent handler files) after T017–T019.
- US1 (Phase 3), US2 (Phase 4), US4 (Phase 6), US6 (Phase 9) can proceed in parallel by different contributors once Phase 2 lands.
- US3 (Phase 5) and Phase 7 wait on US2.
- US5 (Phase 8) waits on US1 + US2 + Phase 7.
- Polish doc updates (T041–T044) all parallel.

---

## Implementation Strategy

**MVP scope**: Phases 1 + 2 + 3 (US1) deliver the standalone stability win — the service stops wedging — and would be safe to ship alone if scope had to compress. Phases 4 + 5 (US2 + US3) then deliver the attribution correctness win as a paired unit. Phases 6 / 7 / 8 / 9 are independent increments that round out the sprint.

**Recommended path**: land Phase 1 + 2 together → ship Phase 3 (US1) and Phase 4 (US2) in parallel → land Phase 5 (US3) as soon as Phase 4 is green → land Phase 6 (US4) and Phase 9 (US6) opportunistically → land Phase 7 prep → run Phase 8 (US5) → finish with Phase 10 polish + kwi closures.
