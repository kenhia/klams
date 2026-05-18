---
description: "Task list for sprint 002 — Safety, Drift Control, and the User View"
---

# Tasks: Safety, Drift Control, and the User View

**Input**: Design documents from `/specs/002-safety-and-write-ops/`
**Prerequisites**: [spec.md](spec.md), [plan.md](plan.md), [research.md](research.md), [data-model.md](data-model.md), [contracts/openapi.yaml](contracts/openapi.yaml), [quickstart.md](quickstart.md)

**Tests**: Integration and contract tests are in scope this sprint (plan §"Testing" names the files explicitly). Test tasks are listed before implementation tasks per the constitution's TDD gate.

**Organization**: Tasks are grouped by user story so each story can be implemented, tested, and demoed independently. Foundational work (migration + shared types + API envelope) blocks every story and lives in Phase 2.

## Format: `[ID] [P?] [Story?] Description`

- **[P]**: Different files, no dependency on incomplete tasks — safe to run in parallel.
- **[Story]**: `[US1]` … `[US5]`. Setup/Foundational/Polish tasks have no story tag.
- File paths are workspace-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Bring tooling to parity with this sprint's needs. The workspace itself is unchanged from 001 — no scaffolding or new crates.

- [X] T001 [P] Add `just` install snippet (Debian/Ubuntu + cargo path) to [docs/setup.md](docs/setup.md) under a new "Developer tooling" subsection so contributors install the recipe runner before any other Phase 2+ task expects it.
- [X] T002 [P] Add a top-level `[decay]` template block (commented placeholders for `task_interval_seconds`, `batch_size`, and per-type λ) to [deploy/config/klams.example.toml](deploy/config/klams.example.toml). Defaults still live in code; the block exists so operators see the knob.

**Checkpoint**: Tooling and config template ready. No service code touched yet.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Ship the migration, shared DTOs, API error envelope, and config-loader extensions that every user story depends on.

**⚠️ CRITICAL**: No US-tagged task may start until this phase is complete.

### Migration

- [X] T003 Author the Phase 2 migration at `migrations/0002_dissents.sql` per [data-model.md](data-model.md): create `dissents` table (all columns + CHECK on `status`), add `dissent_count INT NOT NULL DEFAULT 0` to `facts`, create `dissents_fact_id_idx`, `dissents_status_idx`, `dissents_pending_age_idx`, `dissents_pending_dedupe_idx` (UNIQUE partial), and install `refresh_fact_dissent_count_tg()`, `orphan_pending_dissents_tg()`, and the three triggers (`dissents_after_insert`, `dissents_after_status_update`, `facts_before_delete_orphan_dissents`).
- [X] T004 Add a sqlx migration test in `crates/klams-store/tests/migration_0002.rs` that runs the new migration against a fresh DB and asserts: `dissents` exists with the expected columns, `facts.dissent_count` exists with default 0, the four indexes exist, and the three triggers are listed in `pg_trigger`.

### Shared DTOs (crates/klams-types)

- [X] T005 [P] Extend [crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs) with `ValidationError`, `ErrorDetail`, and `ValidationResult` per data-model.md §"Validator and sanity-rule shapes". Derive `Serialize`, `Deserialize`, `Debug`, `Clone`, `PartialEq`.
- [X] T006 [P] Add `Dissent`, `DissentStatus` (enum `pending|promoted|discarded|orphaned` with `#[serde(rename_all = "snake_case")]`), and `DissentSubmittedResponse { dissent_id: Uuid, fact_id: Uuid, status: DissentStatus, deduped: bool }` to [crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs).
- [X] T007 [P] Add `FactWriteOutcome { Persisted(Fact) | Dissented { dissent_id, fact_id } | VersionConflict { current_version, fact_id } }` to [crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs); update `Fact` struct to include `dissent_count: i32` (default 0).
- [X] T008 Extend the `ApiError` struct in [crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs) with `details: Option<Vec<ErrorDetail>>` and `current_version: Option<i32>` (both `#[serde(skip_serializing_if = "Option::is_none")]` to keep wire compatibility with 001 clients).

### API error envelope + router scaffolding (crates/klams-api)

- [X] T009 In [crates/klams-api/src/error.rs](crates/klams-api/src/error.rs), add error-code constants `VERSION_CONFLICT` (HTTP 409) and `TRUST_REQUIRED` (HTTP 403); add constructors `version_conflict(current_version, fact_id)` and `trust_required(message)`; ensure `validation_error` constructor accepts a `Vec<ErrorDetail>` and emits the new `details` field.
- [X] T010 Register the `/memory/dissents` route group (empty handlers for now, returning 501) in [crates/klams-api/src/router.rs](crates/klams-api/src/router.rs) so US2 work can wire handlers without touching the router file.

### Service config (crates/klams-service)

- [X] T011 Extend [crates/klams-service/src/config.rs](crates/klams-service/src/config.rs) with a `DecayConfig { task_interval_seconds: u64, batch_size: u32, lambda_per_type: HashMap<FactType, f32> }` and parsing from a `[decay]` TOML section; baked-in defaults match plan §"Constraints" (`UserFact = 1e-9`, `TaskFact = 1e-6`, `EnvFact = 1e-9`, `task_interval_seconds = 3600`, `batch_size = 500`). At service startup, log the resolved λ for every `FactType` at INFO.
- [X] T012 Add a config unit test in `crates/klams-service/src/config.rs` (cfg-test module) confirming: (a) missing `[decay]` block loads defaults, (b) partial overrides keep defaults for unconfigured types, (c) full override roundtrips.

**Checkpoint**: Migration applied, types compile, error envelope extended, router scaffold in place, config loader knows about decay. US1–US5 unblocked.

---

## Phase 3: User Story 1 — Reject malformed and untrustworthy agent writes (Priority: P1) 🎯 MVP

**Goal**: Per-type validators + universal sanity rules + hallucination filters land in the API and worker so malformed writes return HTTP 422 with field-level detail and never reach a store.

**Independent Test**: Quickstart §4 — `curl -X POST /memory/facts` with a missing required field, a malformed hostname, and a far-future timestamp each return 422 with the right rule id in `details`; `GET /memory/facts` shows nothing was written.

### Tests for US1 (write FIRST, must FAIL before impl)

- [X] T013 [P] [US1] Extend [crates/klams-api/tests/contract_facts.rs](crates/klams-api/tests/contract_facts.rs) with cases asserting the 422 wire shape: `code=validation_error`, `details[]` contains `{field, rule, message}` entries for missing-field, hostname-shape, timestamp-range, and numeric-range violations against canned `serde_json::Value` payloads.
- [X] T014 [P] [US1] Create `crates/klams-service/tests/us1_validation.rs` end-to-end against `tests/docker-compose.test.yml`: post a missing-`name` UserFact, a bad-hostname TaskFact, a far-future Event, and a structurally-bad numeric range; assert 422 + counts unchanged in Postgres after each. Include one case with `source=User` that trips `hostname_shape` and assert it still returns 422 (FR-006: sanity rules apply to every source). Include one case omitting `expected_version` on a write that targets an existing canonical fact and assert 422 with `details[0].rule = "expected_version_required"` (A1 from /speckit.analyze).

### Implementation for US1

- [X] T015 [P] [US1] Create `crates/klams-core/src/validate/mod.rs` with the `Validator` trait and `ValidatorRegistry { per_type: HashMap<FactType, Vec<Box<dyn Validator>>>, sanity: Vec<Box<dyn Validator>> }`; expose `register_default(&mut self)` that wires every sanity + per-type validator from sibling modules and a `validate_write(&MemoryWrite) -> ValidationResult` entry point.
- [X] T016 [P] [US1] Create `crates/klams-core/src/validate/sanity.rs` with `TimestampRangeRule` (±10y of wall clock, allowlist of field name suffixes per data-model.md), `HostnameShapeRule` (the conservative LDH+dots regex), and `NumericRangeRule` (per-field bounds via metadata).
- [X] T017 [P] [US1] Create `crates/klams-core/src/validate/facts.rs` with one validator function per `FactType` matching the field rules in data-model.md §"Per-type validators" (`UserFact`, `TaskFact`, `EnvFact`).
- [X] T018 [P] [US1] Create `crates/klams-core/src/validate/events.rs` with validators for event categories `Service` and `Execution`; other categories fall through to sanity-only.
- [X] T019 [US1] Wire the registry into [crates/klams-api/src/handlers/facts.rs](crates/klams-api/src/handlers/facts.rs) so the synchronous pre-enqueue path returns HTTP 422 with `details` populated when validation fails. Same wiring in `handlers/events.rs` and `handlers/knowledge.rs` (the latter for envelope-level checks only).
- [X] T020 [US1] In [crates/klams-core/src/worker.rs](crates/klams-core/src/worker.rs), wire the same validator registry into the worker as defense-in-depth (no new validator implementation — reuse the registry from T015) and surface a `WorkerError::Validation(Vec<ErrorDetail>)` that the API layer maps back to 422 if it ever escapes the handler check.
- [X] T021 [US1] Add Prometheus metric `klams_validation_rejections_total{rule}` (incremented per rule id in the API handler) per plan Constitution Check row V.

**Checkpoint**: US1 fully functional — malformed agent writes are rejected with actionable field-level detail. Story 1 acceptance scenarios 1–4 and SC-001 pass.

---

## Phase 4: User Story 2 — Preserve dissenting writes for human review (Priority: P1)

**Goal**: Lower-trust contradictory writes land as `dissents` (HTTP 202); same-trust stale-version writes return 409; promote/discard endpoints let `User`/`Controller` resolve them; `dissent_count` surfaces on every canonical fact read.

**Independent Test**: Quickstart §5 — post a `User`-sourced fact, post a contradicting `AgentProposal`, observe 202 + `dissent_count=1`; list dissents, promote one, observe canonical update + `dissent_count=0`; race two same-trust writes against the same version and observe 409 with `current_version` in the body.

### Tests for US2 (write FIRST, must FAIL before impl)

- [X] T022 [P] [US2] Create `crates/klams-api/tests/contract_dissents.rs` asserting wire shapes for: `POST /memory/facts` 202 response (`DissentSubmittedResponse`), `POST /memory/facts` 409 response (`ApiError { code: "version_conflict", current_version, ... }`), `GET /memory/dissents` page shape, `GET /memory/dissents/{id}`, `POST /memory/dissents/{id}/promote` (200 → updated `Fact`), `POST /memory/dissents/{id}/discard` (200 → resolved `Dissent`), and the 403 `trust_required` shape from an `AgentProposal`-source promote attempt.
- [X] T023 [P] [US2] Create `crates/klams-service/tests/us2_dissents.rs` covering the full lifecycle: persist `User` fact → submit contradicting agent write → assert 202 + dissent row + `dissent_count` trigger fire → list dissents → promote → assert canonical change + `dissent_count` back to 0 + dissent status `promoted` + `resolved_by_source`. Add a second test for the dedupe path (FR-013) and a third for the orphan path (delete canonical with pending dissents). The orphan-path test MUST also assert observability (FR-014): `klams_dissents_total{outcome="orphaned"}` increases by the number of orphaned dissents and a structured log line with `outcome="orphaned"` is emitted.

### Implementation for US2

- [X] T024 [P] [US2] In [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs), add `insert_dissent` (with `ON CONFLICT (fact_id, payload_hash) WHERE status='pending' DO UPDATE` for dedupe; returns id + `deduped: bool`), `list_dissents` (with the OpenAPI's filter set), `get_dissent`, and a struct-mapping helper for the `Dissent` DTO.
- [X] T025 [US2] Add `promote_dissent(dissent_id, caller_source, expected_version)` to [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs): single transaction that asserts the dissent is still `pending` (else 410), checks `facts.version` against `expected_version` (else 409 with current), replaces canonical `payload`/`payload_hash`/`source`, bumps `version`, sets `updated_at`, then updates dissent `status='promoted'` + `resolved_at` + `resolved_by_source`.
- [X] T026 [US2] Add `discard_dissent(dissent_id, caller_source)` to [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs): single UPDATE setting `status='discarded'` only when current status is `pending` (else 410).
- [X] T027 [US2] In [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs), change `upsert_fact` to return `FactWriteOutcome`: when caller source is strictly lower trust than stored source and payload contradicts canonical, call `insert_dissent` and return `Dissented`; when `expected_version` mismatches stored version, return `VersionConflict { current_version }`; otherwise canonical upsert and return `Persisted`. Add `fact_select_for_update_with_version` helper for the version check.
- [X] T028 [US2] In [crates/klams-core/src/worker.rs](crates/klams-core/src/worker.rs), propagate `FactWriteOutcome` from the store through the oneshot reply channel; the worker no longer rejects lower-trust writes, it routes them through `upsert_fact` and forwards whatever variant comes back.
- [X] T029 [US2] In [crates/klams-api/src/handlers/facts.rs](crates/klams-api/src/handlers/facts.rs), map `FactWriteOutcome::Persisted` → 200 + `Fact`, `Dissented` → 202 + `DissentSubmittedResponse`, `VersionConflict` → 409 via the new `ApiError::version_conflict` constructor.
- [X] T030 [P] [US2] Create [crates/klams-api/src/handlers/dissents.rs](crates/klams-api/src/handlers/dissents.rs) with `list`, `get`, `promote`, and `discard` handlers per the OpenAPI. Promote/discard reject any source other than `User` or `Controller` with `ApiError::trust_required` → 403. 410 returned when the targeted dissent is not `pending`.
- [X] T031 [US2] Replace the 501 scaffolding in [crates/klams-api/src/router.rs](crates/klams-api/src/router.rs) with the four real `/memory/dissents` routes wired to `handlers::dissents`.
- [X] T032 [US2] Update [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs) `select_fact` / `list_facts` / `search` paths to project `dissent_count` into the returned `Fact` (column already maintained by triggers from T003). Update the `SearchHit` mapping in [crates/klams-store/src/qdrant.rs](crates/klams-store/src/qdrant.rs) — actually search hits for `type=fact` come back through Postgres, so this is purely the Postgres mapper.
- [X] T033 [P] [US2] Extend [crates/klams-client/src/lib.rs](crates/klams-client/src/lib.rs) with `list_dissents`, `get_dissent`, `promote_dissent`, `discard_dissent`, and change `upsert_fact` to return `FactWriteOutcome` (callers in tests update accordingly).
- [X] T034 [US2] Add Prometheus metrics `klams_dissents_total{outcome}` (where outcome ∈ `accepted,duplicate,promoted,discarded,orphaned`) and `klams_version_conflicts_total` per plan Constitution Check row V.

**Checkpoint**: US2 fully functional — all Story 2 acceptance scenarios + SC-002 + SC-003 pass.

---

## Phase 5: User Story 3 — Decay-aware ranking that fades stale memory (Priority: P2)

**Goal**: A background `tokio` task recomputes `decay_weight` per type from config; `last_used_at` bumps are coalesced from the read path; `/memory/search` ranking incorporates the new weight.

**Independent Test**: Quickstart §6 — `just test -- --test us3_decay` seeds two equal-relevance facts of different types, runs one decay batch with simulated elapsed time, and asserts the Working-typed fact's weight dropped more and search reorders accordingly.

### Tests for US3 (write FIRST, must FAIL before impl)

- [X] T035 [P] [US3] Create `crates/klams-service/tests/us3_decay.rs`: seed one `UserFact` and one `TaskFact` matching a shared search term, set `last_used_at` 7 days into the past on both, invoke the decay task's `tick_once` test entry point with the default config, then assert (a) `TaskFact.decay_weight < UserFact.decay_weight`, (b) `POST /memory/search` returns `UserFact` ahead of `TaskFact` for the shared term, and (c) the `klams_decay_facts_updated_total` counter advanced by 2. Add two more sub-tests covering the spec's Edge Cases: (1) **concurrent-write race** — issue an `upsert_fact` for one of the seeded facts mid-batch (between `tick_once` selecting the row and committing) using a test hook or two-tick interleave, and assert both the payload update and the recomputed `decay_weight` are present post-batch with no clobber; (2) **convergence** — call `tick_once` twice in a row with no intervening read and assert each fact's `decay_weight` is monotonically non-increasing across ticks and does not oscillate.

### Implementation for US3

- [X] T036 [P] [US3] Create [crates/klams-core/src/decay.rs](crates/klams-core/src/decay.rs): `DecayConfig` (re-exported from `klams-types`), `score(base: f32, lambda: f32, age_seconds: f32) -> f32` helper, and `DecayTask { cfg, store }` with `run` (loop) and `tick_once` (single batch, for tests). Batched UPDATE … FROM (VALUES …) per research §4; `id`-ordered iteration with the last processed id retained between batches; `yield_now().await` between batches.
- [X] T037 [US3] In [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs), add `select_decay_batch(after_id, limit)` returning `(id, type, last_used_at, created_at, current_decay_weight)` and `apply_decay_batch(updates: &[(Uuid, f32)])` executing the bulk UPDATE in one round trip.
- [X] T038 [US3] Update the search ranking expression in [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs) (the unified `search` query) so the final ORDER BY incorporates `decay_weight * confidence * (1 + ln(1 + use_count))` — exact form per plan §7. Keep the existing relevance term unchanged.
- [X] T039 [US3] Add `LastUsedBumper` to [crates/klams-core/src/decay.rs](crates/klams-core/src/decay.rs) (or a sibling module): a thin wrapper around `tokio::mpsc::Sender<Uuid>` (capacity 1024) with a `send_lossy` method, plus a `drain_into_batch(rx)` coroutine the decay task awaits between iterations to flush bumps as one UPDATE `last_used_at = now(), use_count = use_count + 1 WHERE id = ANY($1)`. Increment `klams_last_used_bumps_dropped_total` when the channel is full.
- [X] T040 [US3] In [crates/klams-store/src/postgres.rs](crates/klams-store/src/postgres.rs), thread the `LastUsedBumper` handle through every fact-returning read (`select_fact`, `list_facts`, the fact rows in `search`) and invoke `send_lossy(id)` per row returned.
- [X] T041 [US3] In [crates/klams-service/src/main.rs](crates/klams-service/src/main.rs), construct the `LastUsedBumper` and the `DecayTask`, `tokio::spawn` the decay loop, and pass the bumper handle into the `klams-store` builder. Log a one-line `INFO` summary at startup ("decay: interval=…s batch=… lambdas=UserFact=… TaskFact=… EnvFact=…").
- [X] T042 [US3] Add Prometheus metrics `klams_decay_runs_total`, `klams_decay_facts_updated_total`, `klams_last_used_bumps_dropped_total` per plan Constitution Check row V.

**Checkpoint**: US3 fully functional — Story 3 acceptance scenarios 1–4 + SC-004 pass. Existing Phase 1 search latency target (SC-003 from 001) still holds because decay never blocks reads.

---

## Phase 6: User Story 4 — Inspect and curate memory from the viewport (Priority: P2)

**Goal**: The viewport gains a shared provenance panel, per-fact Edit/Delete actions on the existing inspector pages, and a new `/dissents` route that diffs + promotes/discards. Backend pieces from US2/US3 are already shipped.

**Independent Test**: Quickstart §7 — open a fact, see provenance with all eight fields; Edit changes it (User-sourced canonical write); Delete removes it; the Dissents page lists pending dissents, shows a diff, and Promote/Discard each behave as the API contract requires; optimistic updates roll back on backend error and surface the API envelope's `message` + `details`.

### Tests for US4 (write FIRST, must FAIL before impl)

- [X] T043 [P] [US4] Add Vitest cases to [viewport/src/lib/api.test.ts](viewport/src/lib/api.test.ts) (create the file if absent) covering: `listDissents`, `getDissent`, `promoteDissent` (success + 409 + 410 + 403 mapping), `discardDissent`, and the `UpsertResult` discriminated union shape returned by `upsertFact`.
- [X] T044 [P] [US4] Add a `cargo test` unit test under `viewport/src-tauri/src/commands/memory.rs` (cfg-test module) for `list_dissents`, `promote_dissent`, `discard_dissent`, `delete_fact`, and `edit_fact` Tauri command wrappers using the existing trait-mocked `klams-client` from the 001 viewport tests.

### Implementation for US4

- [X] T045 [P] [US4] Extend [viewport/src/lib/types.ts](viewport/src/lib/types.ts) with `Dissent`, `DissentStatus`, `UpsertResult` (discriminated union over `Persisted | Dissented | VersionConflict`), `ProvenanceBundle`, and add `dissentCount` to the existing `Fact` type.
- [X] T046 [P] [US4] Extend [viewport/src/lib/api.ts](viewport/src/lib/api.ts) with `listDissents`, `getDissent`, `promoteDissent`, `discardDissent`, and update `upsertFact` to return the `UpsertResult` union (mapping the 200/202/409 outcomes by HTTP status).
- [X] T047 [P] [US4] Create `viewport/src/lib/ProvenancePanel.svelte`: pure presentational component rendering `ProvenanceBundle` fields with a clear "Pending dissents: N" footer when `dissentCount > 0` (links to `/dissents?fact_id=…`).
- [X] T048 [P] [US4] Create `viewport/src/lib/optimistic.ts`: the per-store snapshot + rollback pattern from research §6 (`withOptimistic(store, prediction, op)` helper).
- [X] T049 [US4] In [viewport/src/routes/facts/+page.svelte](viewport/src/routes/facts/+page.svelte), embed `ProvenancePanel`, add Edit (modal → `User`-sourced `upsertFact` via canonical path) and Delete (confirmation dialog → `deleteFact`) actions, both wired through `withOptimistic`. Surface the API envelope's `message` + `details` in a toast on failure.
- [X] T050 [US4] In [viewport/src/routes/events/+page.svelte](viewport/src/routes/events/+page.svelte), embed `ProvenancePanel` (read-only — events are append-only; no Edit/Delete).
- [X] T051 [US4] In [viewport/src/routes/knowledge/+page.svelte](viewport/src/routes/knowledge/+page.svelte), embed `ProvenancePanel` (read-only — knowledge re-index is the Phase 3 write path).
- [X] T052 [P] [US4] Create `viewport/src/routes/dissents/+page.svelte`: list pending dissents with filter chips (`fact_id`, `source`, age), per-row diff of `proposed_payload` vs canonical, Promote and Discard buttons wired through `withOptimistic` and the new API helpers. Surface 410 (already-resolved) with a soft refresh of the row.
- [X] T053 [US4] Add a top-level "Dissents" navigation link with a badge of the pending count (cheap `listDissents({limit:1, status:'pending'})` on app load + after each mutation) to [viewport/src/routes/+layout.svelte](viewport/src/routes/+layout.svelte).
- [X] T054 [US4] In [viewport/src-tauri/src/commands/memory.rs](viewport/src-tauri/src/commands/memory.rs), add passthrough commands `list_dissents`, `get_dissent`, `promote_dissent`, `discard_dissent`, `delete_fact`, and `edit_fact` delegating to the extended `klams-client` from T033.

**Checkpoint**: US4 fully functional — Story 4 acceptance scenarios 1–5 + SC-005 pass.

---

## Phase 7: User Story 5 — One-command developer inner loop via justfile (Priority: P3)

**Goal**: A `justfile` at the repo root captures every routine developer action; `just gate` is the constitution pre-commit gate and the single command CI invokes.

**Independent Test**: Quickstart §8 — `just --list` shows the eleven recipes; `just gate` runs fmt + clippy + tests and fails non-zero on any failure; `just compose-up && just health` returns green within 30 seconds on a warm host.

### Implementation for US5 (no test phase — recipes are exercised by `just gate` itself)

- [X] T055 [P] [US5] Create the [justfile](justfile) at repo root with recipes: `default` (→ `just --list`), `health` (curl `/healthz` + `scripts/verify-mvp.sh --light`), `compose-up`, `compose-down`, `compose-rebuild` (down + `--no-cache` build + up), `build` (`cargo build -p klams-service --release`), `run` (`cargo run -p klams-service` with logs to stderr), `test` (`cargo test --workspace`), `gate` (fmt-check + clippy `-D warnings` + workspace tests, fail-fast), `viewport-build` (cargo-xwin Windows cross-build), `verify` (full `scripts/verify-mvp.sh`).
- [X] T056 [P] [US5] Extend [scripts/verify-mvp.sh](scripts/verify-mvp.sh) with a `--light` flag that runs only `/healthz` + a single fact write/read round-trip (skips SC-003 perf seeding and the longer SC-007/008/009 walks); document the flag in the script's header comment.
- [X] T057 [US5] Update [.github/workflows/ci.yml](.github/workflows/ci.yml) so the service job's gate step is exactly `just gate` (replacing the inline fmt/clippy/test invocation). Add a `cargo install just --locked` step to the job's setup so the runner has the binary.

**Checkpoint**: US5 fully functional — Story 5 acceptance scenarios 1–4 + SC-006 + SC-007 pass. CI and dev workflow are now driven by the same `just gate`.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Docs catch up, integration walk-through, sprint demo evidence.

- [X] T058 [P] Update [docs/architecture.md](docs/architecture.md): add the dissent path to the write pipeline ASCII diagram, add a "Decay task" box, and append a short "Phase 2 deltas" section linking back to this sprint's plan/spec.
- [X] T059 [P] Update [docs/setup.md](docs/setup.md): `just` install snippet (cross-link to T001's section), the `[decay]` config block with annotated defaults, and a `just --list` quick reference.
- [X] T060 [P] Update [docs/usage.md](docs/usage.md): dissent lifecycle (submit → list → promote/discard), the viewport's provenance panel + Dissents page, and the `just` recipe reference table.
- [X] T061 [P] Add a one-paragraph "Sprint 002 quick reference" + `just --list` snippet to the existing "Running the MVP" section of [README.md](README.md).
- [X] T062 Walk [quickstart.md](quickstart.md) end-to-end against a freshly-rebuilt stack (`just compose-rebuild && just run` in one shell, the §4–§8 curl/viewport steps in another). Record per-step PASS/FAIL evidence as a new "Phase 2 walkthrough" table in [specs/002-safety-and-write-ops/spec.md](specs/002-safety-and-write-ops/spec.md) (mirroring how 001's spec.md captured SC-001..SC-009).
- [X] T063 Run `just gate` from a clean tree; resolve any new clippy / test fallout. Re-run the existing 001 integration suite (`us1_facts.rs` … `us5_health.rs` + `perf_smoke.rs --ignored`) and confirm zero regressions per FR-032.

---

## Dependencies & Execution Order

### Phase dependencies

- **Setup (Phase 1)**: independent — start immediately.
- **Foundational (Phase 2)**: depends on Setup. Blocks every US phase.
- **US1 (Phase 3)**: depends on Foundational. Independent of US2/US3/US4/US5.
- **US2 (Phase 4)**: depends on Foundational. Independent of US1/US3.
- **US3 (Phase 5)**: depends on Foundational. Independent of US1/US2 at the code level; if running serially, runs after US2 because it shares write-path edits in `worker.rs`/`postgres.rs`.
- **US4 (Phase 6)**: depends on Foundational + US2 (dissent endpoints) + US3 (`dissent_count` on reads + decay surfaced fields). Story 4 acceptance scenarios that don't touch dissents can land sooner if US2 slips.
- **US5 (Phase 7)**: depends on Setup only. Can start any time after T001.
- **Polish (Phase 8)**: depends on every preceding US phase being complete.

### Within each user story

- Tests first, fail, then implementation.
- Types/DTOs before stores; stores before handlers; handlers before client; client before viewport.

### Parallel opportunities

- **Phase 1**: T001 ‖ T002.
- **Phase 2**: T005 ‖ T006 ‖ T007 are independent type additions; T008–T012 each touch one file and can interleave; T003 (migration) is the only hard prerequisite for everything after.
- **Phase 3 (US1)**: T013 ‖ T014 ‖ T015 ‖ T016 ‖ T017 ‖ T018 all touch distinct files.
- **Phase 4 (US2)**: T022 ‖ T023 (tests); T024 ‖ T030 ‖ T033 (different files) once the store contract is settled.
- **Phase 6 (US4)**: T045 ‖ T046 ‖ T047 ‖ T048 (types / api / panel component / optimistic helper); T049/T050/T051 are independent pages but share the panel component, so land T047 first.
- **Phase 7 (US5)**: T055 ‖ T056 are independent; T057 depends on T055.
- **Phase 8**: T058 ‖ T059 ‖ T060 ‖ T061 (all docs); T062 is the integration walk-through; T063 is the final gate.

### Suggested MVP scope

User Story 1 (validation + hallucination filters) on top of the 001 baseline is the smallest shippable safety win — it is the slice that protects every downstream feature from poisoned writes. The acceptance criterion that proves the MVP: any malformed agent write produces 422 with field-level detail and never reaches the store (SC-001).

### Parallel example: foundational types

After T003 (migration) lands, three contributors can pick up:

```text
Contributor A → T005 (ValidationError / ErrorDetail)
Contributor B → T006 (Dissent / DissentStatus / DissentSubmittedResponse)
Contributor C → T007 (FactWriteOutcome / Fact.dissent_count)
```

All three edit [crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs); coordinate via small additive diffs (each appends a new section) and merge before T008 (which touches the existing `ApiError` struct in the same file).
