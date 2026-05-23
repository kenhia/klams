# Tasks: Maintenance, Backups, and Ops

**Input**: Design documents from `/specs/006-maintenance-and-backups/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/backup-status-hook.schema.json, quickstart.md

**Tests**: This sprint includes test tasks. The constitution (Principle II) mandates TDD and the spec explicitly requires an integration-test-driven restore exercise (FR-016) plus contract validation against the JSON schema. Test tasks therefore appear ahead of their corresponding implementation tasks in every phase.

**Organization**: Tasks are grouped by user story to enable independent delivery. Setup and Foundational phases unblock all stories; each user-story phase delivers an independently testable increment.

## Format: `[ID] [P?] [Story] Description with file path`

- **[P]**: Can run in parallel with other [P] tasks in the same phase (different files, no incomplete dependencies)
- **[Story]**: US1 / US2 / US3 / US4 / US5; setup/foundational/polish tasks carry no story label

## Path Conventions

Cargo workspace at `/home/ken/src/ai/klams/`. Backup feature spans four crates:

- `crates/klams-types/src/` — config types
- `crates/klams-store/src/backup/` — snapshot + restore mechanics
- `crates/klams-api/src/middleware/` — maintenance-mode HTTP middleware
- `crates/klams-service/src/backup/` — orchestrator, scheduler, hook executor, metrics

Tests live under `tests/integration/` and `tests/fixtures/`. Grafana JSON at `deploy/grafana/klams.json`. Docs in `docs/`.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Stand up the empty module trees, dev fixtures, and `just` recipe stubs so subsequent phases land into a working skeleton.

- [X] T001 Create empty backup module skeleton in `crates/klams-store/src/backup/mod.rs` re-exporting `postgres`, `qdrant`, `retention` submodules (declare with `pub mod` and empty submodule files) and wire `pub mod backup;` into `crates/klams-store/src/lib.rs`
- [X] T002 Create empty backup module skeleton in `crates/klams-service/src/backup/mod.rs` declaring `pub mod scheduler; pub mod lifecycle; pub mod hook; pub mod metrics;` plus empty submodule files; wire `pub mod backup;` into `crates/klams-service/src/lib.rs`
- [X] T003 [P] Create empty maintenance middleware module at `crates/klams-api/src/middleware/maintenance.rs` and wire `pub mod maintenance;` into `crates/klams-api/src/middleware/mod.rs`
- [X] T004 [P] Add `[backup]` block (commented-out, defaults) to `deploy/config/klams.example.toml` per data-model.md "BackupConfig"; pin `enabled = false`
- [X] T005 [P] Add `humantime-serde = "1"` and `jsonschema = "0.18"` (dev-dep, gated by `cfg(test)`) to the relevant `Cargo.toml` files (`klams-types` for humantime-serde, workspace dev-deps for jsonschema) per data-model.md serializer + R-004 hook contract test plan
- [X] T006 [P] Add `just` recipe stubs `backup-once`, `restore-from <date>`, `backup-validate-config`, and `backup-size` to `justfile`; each prints `not yet implemented` and `exit 1` so the surface area is reserved before phases land

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Land the config types, shared state, and sizing fixture that every story phase depends on. NO user story work begins until this phase is complete.

- [X] T007 Write failing unit tests for `BackupConfig` deserialization and validation in `crates/klams-types/src/config.rs` (test module): valid TOML round-trips; invalid `window_start_utc` (`"25:00"`, `"10"`, `""`) returns parse error; defaults for `daily_count=14`, `weekly_count=4`, `same_day_strategy=Suffix`, `status_hook_timeout=10s`; missing `backup_dir` with `enabled=true` is rejected by `BackupConfig::validate()`
- [X] T008 Implement `BackupConfig`, `WindowStartUtc`, `SameDayStrategy`, and `BackupConfig::validate(&self) -> Result<()>` in `crates/klams-types/src/config.rs` per data-model.md; make T007 green
- [X] T009 Wire `klams.toml` loader (existing in `crates/klams-types/src/lib.rs` or equivalent) to surface a `BackupConfig` field on the top-level `KlamsConfig`; update existing config loader unit tests to expect the new field
- [X] T010 [P] Write failing unit tests for `MaintenanceState` in `crates/klams-service/src/backup/mod.rs` (test module): `MaintenanceState::new()` reports `active() == false`; `mark_active(snapshot)` then `active() == true` and `inflight()` returns the snapshot; `clear()` returns to inactive
- [X] T011 [P] Implement `MaintenanceState` and `RunningSnapshot` in `crates/klams-service/src/backup/mod.rs` per data-model.md; make T010 green
- [X] T012 [P] Register and export the five Prometheus series listed in data-model.md "Prometheus series schema" from `crates/klams-service/src/backup/metrics.rs`, using the existing prometheus registry; verify via a smoke unit test that all five series appear in `/metrics` after registration
- [X] T013 [P] Implement `just backup-validate-config` recipe end-to-end: invokes `klams-service --validate-backup-config` (new binary subcommand or flag; see plan.md Source Code tree → klams-service for the three subcommand handlers introduced by this sprint: `--validate-backup-config`, `--run-backup-now`, `--restore-from`) which loads `klams.toml`, runs `BackupConfig::validate()`, prints `OK: ...` or the error and exits with `0` / `2`; add a unit test on the validation path
- [X] T014 Build the Day-0 sizing fixture at `tests/fixtures/scale_loader.rs` per research.md R-009: loads ~10k facts, ~50k events, ~20k knowledge chunks into the test compose stack; expose as a `cargo test --features scale-fixture` gated integration helper; add a one-line README under `tests/fixtures/backup/README.md` pointing at it
- [X] T015 Implement `just backup-size` recipe: brings up the test compose stack if not running, runs the scale fixture from T014, times one `backup::run_once()` invocation, prints the table `kind | bytes | seconds` to stdout, and appends a dated entry to `specs/006-maintenance-and-backups/sizing.md` (creating the file on first run)

> **Phase 2 status:** T007–T015 all landed. T014 (`FixtureScale::large()` + `scale-fixture`-gated `tests/scale_loader.rs`) and T015 (`just backup-size`) were landed as a focused micro-pass after Phase 3, per the original deferral note.

**Checkpoint**: Config types, shared state, metrics registration, and the sizing fixture are all in place. User story phases (P1 → P3) can now proceed in parallel where dependencies allow.

---

## Phase 3: User Story 1 — Unattended nightly backup of klams state (Priority: P1) 🎯 MVP

**Goal**: A backup window configured to run produces both a Postgres dump and a Qdrant snapshot in `backup_dir`, atomically named, retention-pruned, with Prometheus metrics updated — without operator intervention.

**Independent Test**: Per spec.md US1 Independent Test — configure `[backup]` with a window starting within the next minute, wait, observe both files land within ±2 minutes, `.partial` files absent, metric `klams_backup_last_success_timestamp_seconds` matches file mtime.

### Tests for User Story 1

- [X] T016 [P] [US1] Write failing integration test `tests/integration/backup_pg_dump.rs` covering FR-001/FR-002/FR-004: against the test compose Postgres, `klams_store::backup::postgres::dump(&cfg, &date).await` writes `<backup_dir>/postgres-<date>.dump.partial`, atomic-renames to `postgres-<date>.dump` on success, leaves no `.partial` after success; an injected failure (bad credentials) leaves only the `.partial` file (no committed file) and returns an `Err`
- [X] T017 [P] [US1] Write failing integration test `tests/integration/backup_qdrant_snapshot.rs` covering FR-003/FR-004: against the test compose Qdrant, `klams_store::backup::qdrant::snapshot(&cfg, &date).await` posts to the snapshot REST API, streams the file to disk via atomic rename, returns the `BackupArtifact`; on an injected qdrant error (bad collection name) returns `Err` and leaves no committed file
- [X] T018 [P] [US1] Write failing integration test `tests/integration/backup_retention.rs` covering FR-005 + the `same_day_strategy` edge case: given a populated `backup_dir` (synthetic dated fixture files including same-day `<kind>-YYYY-MM-DD-N.{dump,snapshot}` suffixed pairs), `klams_store::backup::retention::prune(&cfg).await` keeps the newest `daily_count` distinct dates + newest `weekly_count` Sundays per kind, treats suffixed-same-day files as the same date for retention purposes (keeping the highest-N copy), deletes the rest, and is a no-op when called without a preceding new successful artifact; mtime is not consulted
- [X] T019 [US1] Write failing integration test `tests/integration/backup_orchestrator.rs` covering FR-001 end-to-end and SC-001: `klams_service::backup::run_once(&deps).await` produces both committed artifacts, updates all five Prometheus series, sets `MaintenanceState::active=true` during the run and `false` after, and writes/removes the `<backup_dir>/lockfile` correctly
- [X] T020 [US1] Write failing integration test slot for SC-001 timing in `tests/integration/backup_scheduler.rs` using `tokio::time::pause()`: schedule against a `window_start_utc` 60 seconds in the simulated future, advance time, observe `run_once` fires within ±2 simulated minutes of the window instant

### Implementation for User Story 1

- [X] T021 [P] [US1] Implement `klams_store::backup::postgres::dump` in `crates/klams-store/src/backup/postgres.rs` per research.md R-001: `tokio::process::Command` spawning `pg_dump -Fc`, `PGPASSWORD` env on child only, write to `.partial`, atomic rename on success, return `BackupArtifact { kind: Postgres, ... }`; make T016 green
- [X] T022 [P] [US1] Implement `klams_store::backup::qdrant::snapshot` in `crates/klams-store/src/backup/qdrant.rs` per research.md R-003: `POST /collections/{name}/snapshots`, then streaming `GET` to `.partial`, atomic rename, optional in-qdrant copy drop; make T017 green
- [X] T023 [P] [US1] Implement `klams_store::backup::retention::prune` in `crates/klams-store/src/backup/retention.rs` per research.md R-006: filename-as-truth date parsing, daily + Sunday-weekly cohort selection, delete-only-after-success guard; make T018 green
- [X] T024 [US1] Implement `klams_service::backup::lifecycle::BackupRun` and `BackupArtifact` structs + state machine in `crates/klams-service/src/backup/lifecycle.rs` per data-model.md (Idle → Running → Finished{ok}); add lockfile write/clear logic and the stale-lockfile recovery path
- [X] T024a [P] [US1] Write failing integration test `tests/integration/backup_lockfile_recovery.rs` covering the spec.md "Service restart mid-backup" edge case: pre-create `<backup_dir>/lockfile` with a dead pid + a `<backup_dir>/postgres-<date>.dump.partial`, start the service (or call the recovery entry point directly), assert (a) the status_hook is invoked once with `event: "failed"`, `error: "service_restarted_mid_backup"`; (b) the lockfile is removed; (c) the `.partial` file is removed; (d) `klams_backup_runs_total{ok="false"}` increments by 1; T024's recovery implementation makes this green
- [X] T025 [US1] Implement `klams_service::backup::run_once` orchestrator in `crates/klams-service/src/backup/mod.rs` wiring T021/T022/T023/T024: flips `MaintenanceState::active=true`, calls Postgres dump then Qdrant snapshot then retention prune, updates metrics, flips `MaintenanceState::active=false` in a guard that fires on both success and failure; make T019 green
- [X] T026 [US1] Implement `klams_service::backup::scheduler::run` in `crates/klams-service/src/backup/scheduler.rs` per research.md R-002: hand-rolled `tokio::time::sleep_until` loop computing the next UTC instant matching `window_start_utc`, calling `run_once` once per day, skipping if `BackupConfig::enabled == false`; make T020 green
- [X] T027 [US1] Wire `scheduler::run` into the `klams-service` main runtime (`crates/klams-service/src/main.rs` or service builder) so it spawns at startup when `[backup] enabled = true`; gracefully skip with a single INFO log when disabled
- [X] T028 [US1] Implement `just backup-once` recipe: invokes a `klams-service --run-backup-now` subcommand (or equivalent) that calls `run_once` directly without the scheduler; supports a hidden `--inject-sleep <secs>` debug flag used by quickstart Section 3

**Checkpoint**: US1 delivers MVP. Nightly backup runs unattended; SC-001 + FR-001…FR-006 are green.

---

## Phase 4: User Story 2 — Restore from yesterday's snapshot (Priority: P1)

**Goal**: A documented + integration-tested restore procedure reproduces production fact/event/knowledge counts from a date-stamped snapshot pair.

**Independent Test**: Per spec.md US2 — clean compose stack, copy yesterday's snapshot pair, run `just restore-from <date>`, observe counts match.

**Dependencies**: T021 + T022 (the snapshot artifacts must exist). Otherwise independent of US3/US4/US5.

### Tests for User Story 2

- [X] T029 [US2] Write failing integration test `tests/integration/restore_roundtrip.rs` covering SC-002 + FR-016 per research.md R-008: seed the scale fixture from T014, run `backup::run_once`, tear down + bring up a fresh compose stack, run `restore::run_from(date)`, assert `SELECT COUNT(*) FROM facts/events/knowledge_items` match and a canonical 10-row sample of facts is identical
- [X] T030 [US2] Write failing integration test in `tests/integration/restore_safety.rs` covering FR-013: restoring against a non-empty Postgres without `--force` returns `Err` and leaves the target untouched; with `--force` succeeds; a truncated dump fails `pg_restore` cleanly with `--single-transaction` rollback semantics so the target remains empty after the failed restore

### Implementation for User Story 2

- [X] T031 [US2] Implement `klams_store::backup::postgres::restore` in `crates/klams-store/src/backup/postgres.rs`: shells out to `pg_restore --single-transaction --clean --if-exists`, credentials via the same `PGPASSWORD` path as T021; returns `Err` on non-zero exit. The `--single-transaction` wrapper ensures a failed restore rolls back both the `--clean` DROP statements and the partial data load atomically — a target that fails mid-restore is left in its pre-call state, which is what T030's "leaves the target untouched" assertion relies on. Make T030's `pg_restore` legs green
- [X] T032 [US2] Implement `klams_store::backup::qdrant::restore` in `crates/klams-store/src/backup/qdrant.rs`: uploads the snapshot via qdrant's recovery endpoint (`PUT /collections/{name}/snapshots/recover` or local-file upload, whichever qdrant 1.12 supports) and waits for the collection status to be `green`
- [X] T033 [US2] Implement `klams_service::backup::restore::run_from(date, force: bool)` in `crates/klams-service/src/backup/restore.rs` (new file; wire into `mod.rs`): resolves `<backup_dir>/{postgres,qdrant}-<date>.{dump,snapshot}`, refuses non-empty target unless `force=true`, calls T031 + T032; make T029 + remaining T030 legs green
- [X] T034 [US2] Implement `just restore-from <date> [--force]` recipe: invokes `klams-service --restore-from <date> [--force]` subcommand calling `restore::run_from`; prints per-step progress to stdout, exits 0 on success, non-zero on any failure with the offending step on stderr

**Checkpoint**: US2 delivers DR confidence. SC-002 + FR-013 + FR-016 green.

---

## Phase 5: User Story 3 — Backup window quiesces non-critical writes (Priority: P2)

**Goal**: While `MaintenanceState::active == true`, non-critical write endpoints respond `503 + Retry-After`; reads and User-source critical writes pass through.

**Independent Test**: Per spec.md US3 — during a backup, `POST /memory/facts` returns 503, `GET /memory/facts` returns 200, `POST /memory/dissents/{id}/promote` from User-source returns 200.

**Dependencies**: T011 (`MaintenanceState`). Decoupled from US1's backup task — the middleware reads the flag regardless of who set it.

### Tests for User Story 3

- [X] T035 [P] [US3] Write failing integration test `tests/integration/maintenance_middleware.rs` covering FR-007 + FR-008 + SC-003: build an axum test app with the maintenance middleware + `MaintenanceState`; set `active=false`, all routes 200; set `active=true`, `POST /memory/facts` returns `503 + Retry-After + {"error":"maintenance_window_active","retry_after_seconds":N}`, `GET /memory/facts` returns 200, `POST /memory/search` returns 200, `POST /memory/context` returns 200
- [X] T036 [P] [US3] Write failing integration test (same file) covering the critical-write exception: a `POST /memory/dissents/{id}/promote` request carrying the `User`-source marker returns 200 while `active=true`; the same endpoint without the User marker returns 503

### Implementation for User Story 3

- [X] T037 [US3] Define a `CriticalWrite` axum route extension marker in `crates/klams-api/src/middleware/maintenance.rs`; export a helper to attach it at router-build time
- [X] T038 [US3] Implement the `maintenance_layer(state: MaintenanceState)` axum `from_fn` middleware in `crates/klams-api/src/middleware/maintenance.rs` per research.md R-005: short-circuits non-GET, non-`CriticalWrite` requests with the 503 envelope when `state.active()`; computes `retry_after_seconds` from `RunningSnapshot::expected_end_at` with a 30s floor; make T035 green
- [X] T039 [US3] Wire `maintenance_layer` into the `klams-api` router in `crates/klams-api/src/lib.rs` (or wherever the router is composed); attach `CriticalWrite` to the dissent promote/discard handlers; verify with T036
- [X] T040 [US3] Extend `/healthz` response shape in `crates/klams-api/src/routes/health.rs` (or equivalent) to include the `maintenance: { active, run_id?, started_at?, expected_end_at? }` block per **FR-018** and data-model.md "HTTP envelope additions"; add a unit/integration test asserting both active and inactive shapes

**Checkpoint**: US3 makes the backup consistent across Postgres + Qdrant. SC-003 + FR-007 + FR-008 green.

---

## Phase 6: User Story 4 — Status hook lifecycle subscription (Priority: P2)

**Goal**: When `[backup] status_hook` is set, klams invokes the executable at `started`, `finished`, and `failed` with a versioned JSON payload on stdin matching `contracts/backup-status-hook.schema.json`. A misbehaving hook never affects the backup.

**Independent Test**: Per spec.md US4 — point `status_hook` at a shell script that appends stdin to a log; run a backup; log contains `started` then `finished`/`failed`. Point at a missing/hanging/exit-1 hook; artifacts still land.

**Dependencies**: T025 (orchestrator emits lifecycle events). Otherwise independent.

### Tests for User Story 4

- [ ] T041 [P] [US4] Write failing contract test `tests/integration/backup_status_hook_schema.rs` per data-model.md: build representative `BackupHookEvent` instances for each of `started` / `finished` / `failed`, serialize to JSON via serde, validate against `specs/006-maintenance-and-backups/contracts/backup-status-hook.schema.json` using the `jsonschema` dev-dep; assert every example in the schema's `examples[]` array also validates
- [ ] T042 [P] [US4] Write failing integration test `tests/integration/backup_status_hook.rs` covering FR-009 happy path **and the SC-004 timing budget**: configure `status_hook` to point at `tests/fixtures/backup/sample-hook.sh` (writes stdin to a temp file); trigger a successful backup; assert the temp file contains exactly two JSON documents in order with matching `run_id`, `event ∈ {started, finished}`, `schema_version: 1`; assert `KLAMS_BACKUP_RUN_ID` and `KLAMS_BACKUP_EVENT` env vars were set on the child; assert the elapsed wall-clock time between `MaintenanceState::active = true` and the `started`-hook child process being spawned is `< 500ms` (leaves headroom under the SC-004 2s budget for the shim's Redis publish)
- [ ] T043 [P] [US4] Write failing integration test (same file) covering FR-010 + SC-005: `status_hook` points at a script that `sleep 600`s — backup completes within `status_hook_timeout + 2s` of the hook invocation (SIGTERM grace), artifacts still land, `klams_backup_hook_invocations_total{ok="false"}` increments by 2; separate sub-tests for missing executable and exit-1 hook with same artifact-landing assertion
- [ ] T044 [P] [US4] Create `tests/fixtures/backup/sample-hook.sh` from quickstart.md Section 1 (writes stdin to a per-invocation temp file derived from `$KLAMS_BACKUP_EVENT`); `chmod +x` via a build script or document it in `tests/fixtures/backup/README.md`

### Implementation for User Story 4

- [ ] T045 [US4] Implement `klams_service::backup::hook::BackupHookEvent` + `HookEventKind` in `crates/klams-service/src/backup/hook.rs` per data-model.md with `serde::Serialize` derives matching the schema; add unit tests verifying the per-event field-presence rules (started: ended_at/duration_ms null, artifacts empty, ok=false; finished: ok=true, error null; failed: ok=false, error required)
- [ ] T046 [US4] Implement `klams_service::backup::hook::invoke(cfg, event) -> InvokeResult` per research.md R-004: `tokio::process::Command` with piped stdin, env passthrough for `KLAMS_BACKUP_RUN_ID` + `KLAMS_BACKUP_EVENT`, `tokio::time::timeout(status_hook_timeout, ...)`, SIGTERM → 2s grace → SIGKILL on timeout; captures stdout/stderr to 4 KiB ring buffers and emits a single `tracing` event per invocation; updates `klams_backup_hook_invocations_total{event, ok}`; never returns `Err` to the caller — hook failure is observability, not control flow; make T041 + T042 + T043 green
- [ ] T047 [US4] Wire `hook::invoke` into the orchestrator from T025: emit `started` before the first artifact begins, emit `finished` after all artifacts succeed, emit `failed` if any artifact failed; ensure a failed `started` invocation does NOT skip the `finished`/`failed` invocation (covered by T043)

**Checkpoint**: US4 surfaces backup lifecycle to any external observer via a generic exec contract. SC-004 (modulo the kpidash shim that lives outside klams) + SC-005 green.

---

## Phase 7: User Story 5 — Grafana dashboard + ansible-k handoff (Priority: P3)

**Goal**: Ship `deploy/grafana/klams.json` covering queue/throughput/latency/backup/maintenance/summarization panels; ship the handoff doc at `~/ansible-k/specs/klams-integration/klams-grafana.md` so ansible-k can operationalize provisioning and alerts.

**Independent Test**: Per spec.md US5 — manually import the JSON into a Grafana scraping the kubs0 exporter; all panels render; "Last backup age" reads `< 26h` post-backup; "Maintenance mode" panel reflects state.

**Dependencies**: T012 + T025 + T038 (the dashboard references the new series; manual smoke needs at least one backup run to populate them).

- [ ] T048 [P] [US5] Author `deploy/grafana/klams.json` covering the panel list in research.md R-007: queue depth + worker utilization, write throughput by endpoint, p50/p95/p99 latency for `/memory/search` and `/memory/context`, error rate by status code, "Last backup age" stat (thresholds 24h green / 26h amber / 48h red), "Maintenance mode" stat, "Summarization lag"; pin datasource UID to `prometheus-default`; folder = `klams`
- [ ] T049 [P] [US5] Add a JSON-syntax smoke test for `deploy/grafana/klams.json`: a tiny integration test (`tests/integration/grafana_dashboard_json.rs`) that parses the file with `serde_json::Value`, asserts every PromQL expression in `panels[].targets[].expr` references only series listed in `~/ansible-k/specs/klams-integration/klams-grafana.md`'s series table (parse the handoff doc's markdown table at test time, treat unknown series as test failure)
- [ ] T050 [US5] Verify `~/ansible-k/specs/klams-integration/klams-grafana.md` (already authored in the prior planning turn) is in sync with the panel set landed by T048: every series the panels reference appears in the handoff's series table; the two recommended alerts (`klams_backup_stale`, `klams_backup_failures`) reference series klams now exports; update the handoff doc if any drift is found
- [ ] T051 [US5] Perform the sprint-internal manual import smoke test per quickstart.md Section 6: import `deploy/grafana/klams.json` into a reachable Grafana, verify every panel renders, capture the result (panel-by-panel checklist) in `specs/006-maintenance-and-backups/sizing.md` (or a new `dashboard-smoke.md` if the sizing file feels overloaded)

**Checkpoint**: US5 delivers operator visibility + the ansible-k handoff. SC-006 + SC-008 green; SC-004 fully closes once ansible-k operationalizes the dashboard on its side.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, the once-exercised restore drill capture, and the final `just gate` pass. Per Constitution IV, docs are part of the definition of done — these tasks are not optional.

- [ ] T052 Update `docs/setup.md` with a new "Restore from snapshot" section per FR-014: verbatim commands matching quickstart.md Section 5, callout that `--force` is required against non-empty targets, link to the quickstart for the once-exercised drill, link to `specs/006-maintenance-and-backups/spec.md` for context
- [ ] T053 [P] Update `docs/usage.md` per Constitution IV: add a `[backup]` config example block matching `klams.example.toml`, document the new just recipes (`backup-once`, `restore-from`, `backup-validate-config`, `backup-size`), document the `503 + maintenance_window_active` error envelope clients should expect during the window, point at `contracts/backup-status-hook.schema.json` and the kpidash shim pattern, document the `/healthz.maintenance` extension
- [ ] T054 [P] Update `docs/architecture.md` per Constitution IV: add a "Backup & maintenance" subsection covering the scheduler, orchestrator, MaintenanceState, hook executor, and the file-system layout in `backup_dir`; tweak the ASCII diagram to add a "Backup task" box next to `klams-service`; and **explicitly add a one-line cross-reference** (under the new subsection) pointing at `~/ansible-k/specs/klams-integration/klams-grafana.md` so SC-008's cross-link assertion is satisfied
- [ ] T055 [P] Capture the executed restore drill (SC-002 + FR-016): record the date the integration test from T029 first ran clean against the scale-fixture data and link to the CI run (or local run output) from a new "FR-016 evidence" note in `specs/006-maintenance-and-backups/quickstart.md` or a sibling `restore-drill.md`
- [ ] T056 [P] Re-evaluate sprint-005's deferred T055/T056 benchmarks (SC-001/SC-003/SC-004 for sprint 005) now that the scale fixture from T014 exists; either land them in this sprint as `tests/integration/sprint005_retrieval_bench.rs` or write a one-line note in `specs/005-advanced-retrieval/tasks.md` updating their status with the fixture-now-exists link
- [ ] T057 Run the full `just gate` (`cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`) per Constitution III; fix any drift; ensure no new clippy allows were introduced
- [ ] T058 Verify the spec.md acceptance checklist (SC-001…SC-008) is fully green by re-reading quickstart.md Sections 1-6 end-to-end against the live build; tick the boxes in the sprint quickstart

---

## Dependencies

```
Phase 1 (Setup) ──▶ Phase 2 (Foundational)
                          │
        ┌─────────────────┼──────────────────────────────┐
        ▼                 ▼                              ▼
  Phase 3 (US1) ──▶ Phase 4 (US2)               Phase 5 (US3)
        │                 │                              │
        │                 │                              ▼
        ├─────────────────┼──────────────────────▶ Phase 6 (US4)
        │                 │                              │
        ▼                 ▼                              ▼
                       Phase 7 (US5)
                          │
                          ▼
                       Phase 8 (Polish)
```

Notes on cross-phase coupling:

- **US1 → US2**: US2 reuses the postgres/qdrant snapshot modules from US1 (T021/T022) for the restore-validation test fixture. US2's test (T029) also requires the scale fixture from Foundational T014.
- **US1, US3, US4 in parallel**: US3 (middleware) and US4 (hook) read `MaintenanceState` and lifecycle events respectively; both consume contracts US1's orchestrator (T025) defines but can be developed against unit-test stubs while T025 lands.
- **US5 → all earlier**: dashboard smoke needs metrics emitting (T012 + T025 + at least one orchestrated run); the handoff doc was authored during the prior planning turn and only needs a sync check in T050.
- **Polish → all**: documentation, drill capture, and the `just gate` close the sprint.

## Parallel execution examples

Within Foundational once T007/T008/T009 are done:

- T010 [P], T012 [P], T013 [P], T014 can be picked up by different worktrees / agents; T011 follows T010 in the same file.

Within US1, after the orchestrator skeleton T024 lands, the snapshot modules are independent files:

- T021 [P], T022 [P], T023 [P] can all proceed in parallel (different files, no cross-deps); T025 integrates them.

Within US4, the hook tests are file-disjoint from the implementation:

- T041 [P], T042 [P], T043 [P] (different test files / functions) land first; T045 then T046 implement.

Within Polish, the doc tasks are file-disjoint:

- T052, T053 [P], T054 [P], T055 [P], T056 [P] can fan out to parallel writers; T057 + T058 serialize at the end.

## Implementation strategy

**MVP**: Phases 1 + 2 + 3 alone deliver Story 1 — unattended nightly backups — which is the single Phase 5 exit criterion the master plan calls out by name ("backup runs nightly without intervention"). Everything from Phase 4 onward is incremental delivery on top of a working MVP.

**Suggested order**:

1. **Sprint week 1**: Setup + Foundational (Phases 1-2), including the T0 sizing fixture (T014).
2. **Sprint week 2**: US1 (Phase 3) → MVP. Cut a checkpoint here.
3. **Sprint week 3**: US2 + US3 + US4 in parallel (Phases 4-6) — three small workstreams with file-level independence.
4. **Sprint week 4**: US5 (Phase 7) + Polish (Phase 8).

**Constitution cross-check**:

- **I. SDD**: Every task references its FR/SC ID and the design doc that authorizes it.
- **II. TDD**: Each phase opens with failing tests; implementation tasks explicitly call out the test IDs they make green.
- **III. Code Standards**: T057 runs the unchanged `just gate` as the exit gate.
- **IV. Documentation**: T052/T053/T054 land in the same sprint as the code, not as follow-ups.
- **V. Quality & Observability**: T012 + T046 + T040 cover metrics, structured logging, and `/healthz` extension.
- **VI. Simplicity & Intentional Design**: No new crates, no new persistence, no premature cloud-sync abstraction; scheduler is hand-rolled (R-002), hook is exec-with-JSON (R-004), retention is filename-as-truth (R-006).

## Format validation

All 59 tasks above follow `- [ ] T### [P?] [Story?] Description with file path`:

- Setup (T001-T006): no story label ✓
- Foundational (T007-T015): no story label ✓
- US1 (T016-T028 + T024a): [US1] label ✓
- US2 (T029-T034): [US2] label ✓
- US3 (T035-T040): [US3] label ✓
- US4 (T041-T047): [US4] label ✓
- US5 (T048-T051): [US5] label ✓
- Polish (T052-T058): no story label ✓

Every implementation task names a file path. Every test task names its target file plus the FR/SC IDs it covers.
