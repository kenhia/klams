# Tasks: Non-Agentic Writes, Integrations, and the Systemd Switchover

**Input**: Design documents from [/specs/003-non-agentic-writes/](.)
**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md), [data-model.md](data-model.md), [contracts/](contracts/)

**Tests**: included (the klams constitution mandates TDD — see `.specify/memory/constitution.md` Principle II). Every new endpoint, validator, binary, and deploy artifact lands behind a failing test first.

**Organization**: tasks grouped by user story so each story is independently shippable and testable. Setup + Foundational phases run first; story phases can interleave once Phase 2 closes.

## Format

`- [ ] TID [P?] [USN?] Description with file path(s)`

- `[P]`: parallelizable (different file, no dependency on incomplete tasks).
- `[USN]`: user-story tag (`[US1]`..`[US6]`); omitted for shared phases.

## Path conventions

Rust workspace at repo root. New binary crates land under `crates/klams-scanner/` and `crates/klams-monitor/`. Existing crates are edited in place. Deploy artifacts under `deploy/`. Spec artifacts under `specs/003-non-agentic-writes/`.

---

## Phase 1: Setup (Shared Infrastructure)

- [X] T001 Create the two new binary crates: [crates/klams-scanner/Cargo.toml](crates/klams-scanner/Cargo.toml) and [crates/klams-monitor/Cargo.toml](crates/klams-monitor/Cargo.toml). Each declares one `[[bin]]` matching the crate name, depends on `klams-types`, `klams-client`, `tokio`, `clap`, `tracing`, `serde`, `reqwest`, plus per-crate extras (`ignore`, `rusqlite { features = ["bundled"] }`, `sha2` for scanner). Add both to the workspace `members` list in [Cargo.toml](Cargo.toml).
- [X] T002 [P] Scaffold empty `lib.rs` + `main.rs` for both new crates with a single `#[tokio::main] async fn main()` that logs "klams-scanner ready" / "klams-monitor ready" and exits 0. Confirms the workspace builds with `cargo build --workspace --bins`.
- [X] T003 [P] Create [deploy/](deploy/) directory layout (the directory already exists with `deploy/config/`; this task adds the unit-file siblings): empty placeholders for `deploy/klams-service.service`, `deploy/klams-scanner.service`, `deploy/klams-scanner.timer`, `deploy/klams-monitor.service`, and `deploy/install-systemd.sh` (mode 0755). Files contain `# TODO sprint 003` comments only at this point — they exist so subsequent tasks have a target file to edit.

**Checkpoint**: workspace builds with the two new binary targets; `deploy/` skeleton present; no behavior change yet.

---

## Phase 2: Foundational (Blocking Prerequisites)

> Everything in this phase MUST land before any user-story work begins. It is the contract surface and the schema delta that US1/US3/US5 all depend on.

- [X] T004 Add the additive migration [migrations/0003_events_task_idx.sql](migrations/0003_events_task_idx.sql) per [data-model.md §4](data-model.md). Uses `CREATE INDEX CONCURRENTLY IF NOT EXISTS …` on the expression `(payload->>'task_id'), created_at` with the `WHERE category IN ('Execution','Service')` predicate; Down section uses `DROP INDEX CONCURRENTLY IF EXISTS`. CONCURRENTLY cannot run inside a transaction — mark both files with the `-- @no-transaction` directive on the first line, and verify the `klams-store` migrator detects it and skips BEGIN/COMMIT (extend the migrator if it does not). Wire into the existing `klams-store` migrator and verify by `cargo test -p klams-store`.
- [X] T005 [P] Define `PolicyEntry` and `PolicyTable` in a new module [crates/klams-core/src/policy.rs](crates/klams-core/src/policy.rs) per [data-model.md §5](data-model.md), with `#[derive(Serialize)]` and a `PolicyTable::default()` returning the production values from the table in the same section. Re-export from `klams-core::lib`. Add unit test `default_table_has_strictly_descending_ranks`.
- [X] T006 [P] Add unit test in [crates/klams-core/src/policy.rs](crates/klams-core/src/policy.rs) `policy_table_serializes_to_keyed_object` that asserts `serde_json::to_value(PolicyTable::default())` produces exactly the four top-level keys `User`, `Controller`, `Task`, `AgentProposal` and that each entry has `rank` + `description`. (This is the no-drift unit that backs SC-005.)
- [X] T007 [P] Refactor the existing source-trust comparison in the Phase 2 dispatcher (likely in `klams-core/src/dispatcher.rs` or wherever the Phase 2 dissent code routes) to consult `PolicyTable` rather than hardcoded match arms. Existing Phase 2 behavior MUST be byte-identical (`us2_dissents` integration tests still green); only the source of truth moves.
- [X] T008 [P] Add the `WritePath` enum (`Canonical`, `Dissent`) to [crates/klams-types/src/responses.rs](crates/klams-types/src/responses.rs) with `#[serde(rename_all = "lowercase")]`. Extend the existing `FactWriteResponse`, `EventWriteResponse`, and `KnowledgeIndexResponse` structs with `pub path: WritePath` (always serialized) and `#[serde(skip_serializing_if = "Option::is_none")] pub dissent_id: Option<Uuid>`. Update `klams-client` to surface the new fields on its mirror types.
- [X] T009 Add Prometheus metric `klams_writes_total` with labels `{type, source, path}` in [crates/klams-core/src/metrics.rs](crates/klams-core/src/metrics.rs). Register on startup. Bump the counter inside the existing write handlers (single call site per endpoint) so every code path increments exactly once.
- [X] T010 [P] Update [crates/klams-api/tests/contract_facts.rs](crates/klams-api/tests/contract_facts.rs) to assert the new `path` field is present on 200 responses (`path == "canonical"`, no `dissent_id`). Update [crates/klams-api/tests/contract_events.rs](crates/klams-api/tests/contract_events.rs) and [crates/klams-api/tests/contract_search.rs](crates/klams-api/tests/contract_search.rs) the same way for their write paths. These tests MUST fail before T008/T009 land (they are the TDD red bar).
- [X] T010a [P] Add `?contract=v1` support to the existing `/healthz` handler in [crates/klams-api/src/handlers/health.rs](crates/klams-api/src/handlers/health.rs): when the query string contains `contract=v1`, the JSON body MUST include a top-level `"contract": "v1"` field alongside the existing `status` snapshot; absent or other values keep current behavior byte-identical. Backs FR-021 / the handoff's drift-detection contract. Add two rows to [crates/klams-api/tests/contract_health.rs](crates/klams-api/tests/contract_health.rs): `healthz_with_contract_v1_includes_contract_field` (write FIRST, fails until the handler is updated) and `healthz_without_contract_query_unchanged` (regression guard for back-compat).
- [X] T010b [P] Add `POST /memory/knowledge/delete` endpoint to [crates/klams-api/src/handlers/knowledge.rs](crates/klams-api/src/handlers/knowledge.rs) + route in [crates/klams-api/src/router.rs](crates/klams-api/src/router.rs): accepts `?source_file=<abs_path>` query param, deletes all `knowledge_items` chunks whose payload `source_file` matches, returns `{"deleted": <count>, "path": "canonical"}`. Required by the scanner's delete-on-missing logic (FR-008, T037) \u2014 no Phase 1 endpoint exists today. Add contract rows to [crates/klams-api/tests/contract_knowledge.rs](crates/klams-api/tests/contract_knowledge.rs): `knowledge_delete_requires_bearer`, `knowledge_delete_removes_matching_chunks`, `knowledge_delete_missing_source_file_returns_zero`. Tests written FIRST.

**Checkpoint**: all contract tests pass; `klams_writes_total` counter visible at `/metrics`; PolicyTable backs the dispatcher; index migration applied; `/healthz?contract=v1` and `/memory/knowledge/delete` live. US1/US3/US5 can start.

---

## Phase 3: User Story 5 — Per-source policy enforced and visible (Priority: P2)

> Sequenced first among the story phases because it ships the smallest API delta and unblocks the assertions in US1, US3, and the handoff contract tests.

**Goal**: `GET /memory/policy` exposes the source-trust table and every write response carries `path`. Phase 2 closes by integration-testing both end-to-end.

**Independent Test**: `curl -sH "Authorization: Bearer $K_TOK" http://127.0.0.1:7777/memory/policy` returns the keyed JSON from [contracts/memory_policy.md](contracts/memory_policy.md); a `POST /memory/facts` returns a body containing `path: "canonical"`.

### Tests for US5 (write FIRST, ensure RED)

- [X] T011 [P] [US5] Create [crates/klams-api/tests/contract_policy.rs](crates/klams-api/tests/contract_policy.rs) with the four contract tests listed in [contracts/memory_policy.md § Contract tests](contracts/memory_policy.md#contract-tests): `policy_endpoint_returns_all_four_sources`, `policy_endpoint_ranks_are_strictly_descending`, `policy_endpoint_requires_bearer`, `policy_endpoint_matches_dispatcher`. All four MUST fail at this point (handler does not exist yet).

### Implementation for US5

- [X] T012 [US5] Add `pub async fn policy_handler(State(app): State<AppState>) -> Json<PolicyTable>` in [crates/klams-api/src/handlers/mod.rs](crates/klams-api/src/handlers/mod.rs) (or a new `handlers/policy.rs` — implementer's choice; keep one file per logical group). The handler reads `app.policy_table.clone()` (an `Arc<PolicyTable>` added to `AppState` in this task).
- [X] T013 [US5] Wire the route `.route("/memory/policy", get(policy_handler))` into [crates/klams-api/src/router.rs](crates/klams-api/src/router.rs), behind the existing bearer middleware layer. Re-run T011 tests — all green.
- [X] T014 [US5] Add `pub fn policy(&self) -> Result<PolicyTable>` to [crates/klams-client/src/lib.rs](crates/klams-client/src/lib.rs) so non-Rust callers and integration tests have a typed handle. Cover with one unit test that uses `wiremock` to stub the endpoint and parse the response.
- [X] T015 [US5] Add `crates/klams-service/tests/us3a_policy_endpoint.rs` (`#[ignore]`-gated against the test docker stack) covering: (a) policy endpoint returns the right table, (b) `klams_writes_total{path="canonical"}` increments after one successful fact write, (c) `klams_writes_total{path="dissent"}` increments after one diverted write. Mirrors the structure of existing `us2_dissents.rs`.

**Checkpoint**: `GET /memory/policy` live; every write response carries `path`; SC-005 evidenced by `us3a_policy_endpoint.rs` running green.

---

## Phase 4: User Story 1 — Ansible plays publish host facts (Priority: P1)

> Server-side only — the play-side wiring belongs to the ansible-k repo and is covered by US6's handoff. This phase ensures klams accepts `source=Task` + `task_id` writes correctly and dedupes them.

**Goal**: a `source=Task` write with a `task_id` payload field round-trips through canonical-path handling; rerunning the same payload produces zero new versions.

**Independent Test**: the integration test in T018 (cannot test the actual ansible-k plays here; that's SC-001's job after the ansible-k owner runs them).

### Tests for US1 (write FIRST)

- [X] T016 [P] [US1] Add a failing unit test to `klams-core/src/validate/facts.rs` (file already exists from sprint 002): `ansible_task_id_uuid_or_prefixed_run_id_passes`, `ansible_task_id_too_long_rejected_422`. Validates the regex from [data-model.md §1](data-model.md).
- [X] T017 [P] [US1] Add an integration test [crates/klams-service/tests/us3b_ansible_facts.rs](crates/klams-service/tests/us3b_ansible_facts.rs) covering the three US1 acceptance scenarios using the test docker stack. Mirror sprint-002's `us2_dissents.rs` setup. Tests MUST fail initially (validator changes not yet in place).

### Implementation for US1

- [X] T018 [US1] Extend `klams-core::validate::facts::TaskFactValidator` and `EnvFactValidator` to recognize the optional `task_id` field, apply the regex from [data-model.md §1](data-model.md), and length-cap at 64. Unchanged behavior when `task_id` is absent (Phase 1 controller traces keep working). Re-run T016 — green.
- [X] T019 [US1] In the dispatcher (modified in T007), ensure `source=Task` with `task_id` traverses the canonical path against same-or-lower-trust rows and diverts against higher-trust (User) rows. Verify by re-running T017 — green.
- [X] T020 [US1] Include `task_id` (when present) in the canonical-hash key used for Phase 1 dedupe (`klams-core/src/dedupe.rs` or equivalent). Add a regression unit test asserting two requests with identical payload but different `task_id` produce different canonical hashes — and conversely, two requests with identical payload **and** identical `task_id` produce the same hash (which is what enables the play-rerun-no-new-version behavior).

**Checkpoint**: `us3b_ansible_facts.rs` green; SC-001 testable end-to-end once the ansible-k handoff is implemented in sprint US6.

---

## Phase 5: User Story 3 — Service monitors and execution traces (Priority: P2)

**Goal**: events with `category=Service` and `category=Execution` are validated, indexed efficiently, and a `klams-monitor` binary polling `systemctl` emits the lifecycle events.

**Independent Test**: `systemctl restart qdrant` followed within 60s by `GET /memory/events?category=Service&service=qdrant&since=-2m` returns a `service.down` then `service.up`.

### Tests for US3 (write FIRST)

- [X] T021 [P] [US3] Add unit tests to a new module `crates/klams-core/src/validate/events.rs` for `ServiceEventValidator` and `ExecutionTraceEventValidator`: required-field checks, enum constraint on `event` and `phase`, unknown-value rejection. Mirrors the Phase 2 facts validators.
- [X] T022 [P] [US3] Add unit tests in [crates/klams-monitor/tests/state_diff.rs](crates/klams-monitor/tests/state_diff.rs) covering the state-diff table in [data-model.md §7](data-model.md): transitions produce the expected event; steady-state polls produce nothing; version-change detection.
- [X] T023 [P] [US3] Add integration test [crates/klams-service/tests/us3c_events.rs](crates/klams-service/tests/us3c_events.rs) (`#[ignore]`-gated): posts a `category=Service` event and a `category=Execution` event with the same `task_id`, then queries `events` by `task_id` and asserts the per-task query uses the new index (verify with `EXPLAIN` over sqlx — or simply assert response time is sub-10ms on a 10k-row table).

### Implementation for US3 — validators and index

- [X] T024 [US3] Implement `ServiceEventValidator` and `ExecutionTraceEventValidator` in `crates/klams-core/src/validate/events.rs` per [data-model.md §3](data-model.md) and [§4](data-model.md). Wire them into the existing `EventDispatcher` validate switch.
- [X] T025 [US3] Add `GET /memory/events?category=&service=&task_id=&since=` query params to the existing events handler in [crates/klams-api/src/handlers/events.rs](crates/klams-api/src/handlers/events.rs). Filter by the new index. Add a contract test row.

### Implementation for US3 — klams-monitor binary

- [X] T026 [P] [US3] Implement `crates/klams-monitor/src/poll.rs`: `pub async fn is_active(unit: &str) -> Result<UnitState>` shelling out to `systemctl is-active <unit>` via `tokio::process::Command`. Map exit code 0 → `Active`, non-zero → `Inactive`. Add unit test that uses `which` to verify `systemctl` is on `PATH` and otherwise skips.
- [X] T027 [P] [US3] Implement `crates/klams-monitor/src/state.rs`: `PreviousState` cache + `pub fn diff(prev: &PreviousState, current: &UnitState) -> Option<ServiceEventPayload>` per [data-model.md §7](data-model.md). Covers T022's tests.
- [X] T028 [US3] Implement `crates/klams-monitor/src/publish.rs`: `reqwest` client posting the diff payloads to `POST /memory/events`. Reuse `klams-client` (extend it if needed; do not duplicate the http wiring).
- [X] T029 [US3] Implement `crates/klams-monitor/src/main.rs`: clap config (`--config <toml>`, `--once`, `--interval-secs <u64>`), config TOML shape `{ url, token, units = ["qdrant", "postgresql", "klams-service", ...], interval_secs = 15 }`, main loop calling `poll → diff → publish` on each tick. Structured `tracing` per poll.

**Checkpoint**: `us3c_events.rs` green; `klams-monitor --once` against a running klams successfully posts on a hand-triggered state change; SC-003 testable.

---

## Phase 6: User Story 2 — Repo and notes scanner (Priority: P1)

**Goal**: `klams-scanner` walks `~/src` and `~/obsidian`, chunks changed files, indexes them via `POST /memory/knowledge/index`, and deletes vanished files within one scan cycle.

**Independent Test**: drop a unique-nonce note in `~/obsidian/`, run `klams-scanner --once`, search the nonce, get one hit. Repeat after editing → new content visible. Repeat after deletion → zero hits.

### Tests for US2 (write FIRST)

- [ ] T030 [P] [US2] Add unit tests in [crates/klams-scanner/tests/walk_diff.rs](crates/klams-scanner/tests/walk_diff.rs) covering: gitignore respected, `.klamsignore` respected, `target/` and `.git/` always skipped, mtime-pre-filter avoids hashing unchanged files, vanished files trigger deletion.
- [ ] T031 [P] [US2] Add unit tests in `crates/klams-scanner/src/chunk.rs` covering: paragraph bounds ≈ 800 chars w/ 200 overlap, markdown heading is a hard break, sha256 stable across whitespace normalization.
- [ ] T032 [P] [US2] Add unit tests in `crates/klams-scanner/src/cursor.rs` covering: upsert/select round-trip, mid-walk crash leaves prior progress intact (test by partial-commit then re-open), delete-on-missing logic.
- [ ] T033 [P] [US2] Add integration test [crates/klams-service/tests/us3d_scanner_e2e.rs](crates/klams-service/tests/us3d_scanner_e2e.rs) (`#[ignore]`-gated) covering the three US2 acceptance scenarios end-to-end against the test docker stack, using a `tempfile::TempDir` as the scanner root.

### Implementation for US2

- [ ] T034 [P] [US2] Implement `crates/klams-scanner/src/walk.rs` using the `ignore` crate: builder honors `.klamsignore`, always skips `target/`, `node_modules/`, `.git/`, `__pycache__/`, `.venv/`, returns an iterator of `(absolute_path, mtime_ns, file_size)`.
- [ ] T035 [P] [US2] Implement `crates/klams-scanner/src/chunk.rs`: read file, normalize whitespace, split on paragraph boundaries with markdown-heading hard breaks, target ~800 chars w/ 200 overlap, sha256 each chunk.
- [ ] T036 [P] [US2] Implement `crates/klams-scanner/src/cursor.rs` per [data-model.md §6](data-model.md): rusqlite-bundled, single-table schema, atomic per-file transaction, `upsert/get/delete/list_all` API.
- [ ] T037 [US2] Implement `crates/klams-scanner/src/publish.rs`: reuse `klams-client` to POST `ScannedKnowledgeChunk` payloads (shape from [data-model.md §2](data-model.md)) to `/memory/knowledge/index`; POST deletes to the `/memory/knowledge/delete?source_file=<abs_path>` endpoint added in T010b for vanished files.
- [ ] T038 [US2] Implement `crates/klams-scanner/src/main.rs`: clap (`--config <toml>`, `--once`, `--root <path>` for ad-hoc), config TOML `{ url, token, roots = ["~/src", "~/obsidian"], interval_secs = 3600, state_dir = "~/.local/state/klams" }`, main loop: walk → diff against cursor → chunk → publish → upsert cursor → handle deletions.
- [ ] T039 [US2] Add Prometheus metrics in `klams-scanner` per FR-007: `klams_scanner_files_processed_total`, `klams_scanner_files_skipped_total{reason}`, `klams_scanner_chunks_indexed_total`, `klams_scanner_last_run_timestamp_seconds`. Bind to a `--metrics-listen <addr>` flag so the systemd unit can scrape it.

**Checkpoint**: `us3d_scanner_e2e.rs` green; one-shot `klams-scanner --once --root <tempdir>` indexes a fresh file, re-runs are idempotent, deletes propagate; SC-002 testable on kubs0.

---

## Phase 7: User Story 4 — systemd switchover on kubs0 (Priority: P1)

**Goal**: `klams-service`, `klams-scanner` (via timer), and `klams-monitor` all run under systemd on `kubs0`, survive reboot, and offer the prev-binary rollback path. `just run` stays as the foreground debugger.

**Independent Test**: reboot test (`systemctl is-active` after reboot) + deliberate-bad-build rollback test.

### Tests for US4 (write FIRST)

- [ ] T040 [P] [US4] Add a shell-test [deploy/tests/install-systemd.bats](deploy/tests/install-systemd.bats) (or a Rust harness if `bats` isn't installed — implementer's choice) covering: dry-run mode prints the actions it would take, repeated runs are idempotent, missing dependency (`postgresql.service` absent) fails with a clear error, prev-binary rotation produces both `klams-service` and `klams-service.prev` after an upgrade.
- [ ] T041 [P] [US4] Add `cargo test --test deploy_unit_files` lints (or a simple `cargo xtask` if needed) that asserts each `*.service` file under `deploy/` parses with `systemd-analyze verify` (skipped when `systemd-analyze` isn't on `PATH`).

### Implementation for US4

- [ ] T042 [US4] Author [deploy/klams-service.service](deploy/klams-service.service) per [plan.md "Constraints"](plan.md#technical-context): `After=postgresql.service qdrant.service`, `Requires=` same, `User=klams`, `Group=klams`, `Restart=on-failure`, `RestartSec=5`, `ExecStart=/usr/local/bin/klams-service`, `Environment=KLAMS_CONFIG=/etc/klams/klams.toml`, `StateDirectory=klams`, `NoNewPrivileges=true`, `ProtectSystem=strict`, `ProtectHome=true`.
- [ ] T043 [P] [US4] Author [deploy/klams-scanner.service](deploy/klams-scanner.service) (`Type=oneshot`, same hardening, `ExecStart=/usr/local/bin/klams-scanner --once`) and [deploy/klams-scanner.timer](deploy/klams-scanner.timer) (`OnBootSec=5min`, `OnUnitActiveSec=1h`, `Persistent=true`).
- [ ] T044 [P] [US4] Author [deploy/klams-monitor.service](deploy/klams-monitor.service) (`Type=simple`, `Restart=on-failure`, `ExecStart=/usr/local/bin/klams-monitor`, same hardening).
- [ ] T045 [US4] Author [deploy/install-systemd.sh](deploy/install-systemd.sh): POSIX `sh`, idempotent. Steps: (1) `getent passwd klams || useradd --system --no-create-home --shell /usr/sbin/nologin klams`, (2) `install -d -o klams -g klams /var/lib/klams /etc/klams`, (3) for each binary: copy to `/tmp/klams-stage-$$`, then `mv -f /usr/local/bin/<bin> /usr/local/bin/<bin>.prev` (ignore failure if missing), then `mv -f /tmp/klams-stage-$$/<bin> /usr/local/bin/<bin>` (atomic), (4) `install -m 0644 deploy/*.service deploy/*.timer /etc/systemd/system/`, (5) `systemctl daemon-reload && systemctl enable --now klams-service klams-scanner.timer klams-monitor`. Supports `--dry-run`.
- [ ] T046 [US4] Add `just install-systemd`, `just scanner-once`, `just monitor-once`, and `just rollback` recipes to [justfile](justfile). `just rollback` swaps `klams-service.prev` back into place and restarts the unit. Update the `just --list` snippet in `docs/setup.md` and `README.md` to mention them (deferred to Phase 9 polish to avoid churn here).
- [ ] T047 [US4] Update [crates/klams-service/src/main.rs](crates/klams-service/src/main.rs) to honor `KLAMS_CONFIG` env (it likely already does — verify and add a regression test if not). Confirm `just run` and `systemctl start klams-service` use the same config-load path.

**Checkpoint**: T040/T041 green; install-systemd dry-run prints sensible actions; reboot smoke + rollback test runnable on kubs0 per quickstart §6.

---

## Phase 8: User Story 6 — Handoff document for ansible-k (Priority: P1)

**Goal**: a self-contained handoff under `specs/003-non-agentic-writes/handoff/` ready to `cp -r` to `/home/ken/ansible-k/specs/klams-integration/`.

**Independent Test**: the four contract tests in T048 + a 10-minute cold-read test (SC-006) Ken runs manually.

### Tests for US6 (write FIRST)

- [ ] T048 [P] [US6] Add [crates/klams-service/tests/us3e_handoff_layout.rs](crates/klams-service/tests/us3e_handoff_layout.rs) (file-system only — no `#[ignore]` needed) implementing the four contract tests from [contracts/handoff_index.md § Acceptance](contracts/handoff_index.md#acceptance--contract-tests-in-this-sprint): `handoff_directory_layout_matches_contract`, `handoff_pinned_version_header_present`, `handoff_api_contract_lists_required_failure_modes`, plus a placeholder for `handoff_example_script_posts_userfact` that skips when `KLAMS_URL` is unset and runs the script otherwise.

### Implementation for US6

- [ ] T049 [P] [US6] Author [specs/003-non-agentic-writes/handoff/README.md](specs/003-non-agentic-writes/handoff/README.md) with the pinned-version header verbatim from [contracts/handoff_index.md § README.md](contracts/handoff_index.md#readmemd), the one-paragraph orientation, the TL;DR table, and the read order.
- [ ] T050 [P] [US6] Author [specs/003-non-agentic-writes/handoff/spec.md](specs/003-non-agentic-writes/handoff/spec.md): speckit-compatible spec for the ansible-k side per [contracts/handoff_index.md § spec.md](contracts/handoff_index.md#specmd). Minimum: one P1 user story for the callback-plugin wiring; FRs covering payload shape, dedupe expectations, failure-mode handling; SCs that ansible-k will measure.
- [ ] T051 [P] [US6] Author [specs/003-non-agentic-writes/handoff/api-contract.md](specs/003-non-agentic-writes/handoff/api-contract.md): endpoint table, auth model, minimal valid payload examples (UserFact + EnvFact + Service event), dedupe semantics, failure-mode table (must include all six rows from the contract), integration-shape recommendation, drift-detection section.
- [ ] T052 [P] [US6] Author [specs/003-non-agentic-writes/handoff/examples/post-userfact.sh](specs/003-non-agentic-writes/handoff/examples/post-userfact.sh): POSIX `sh`, `chmod +x`, `KLAMS_URL`/`KLAMS_TOKEN` env-driven with documented defaults, posts a minimal `UserFact`, pretty-prints response. Verify it runs green against the local test stack.

**Checkpoint**: `us3e_handoff_layout.rs` green; the handoff directory is ready to ship.

---

## Phase 9: Polish & Cross-Cutting Concerns

- [ ] T053 [P] Update [docs/architecture.md](docs/architecture.md): new §3 entry for the scanner + monitor + systemd topology; ASCII diagram showing `klams-scanner.timer → klams-scanner → POST /memory/knowledge/index → klams-service`; same for `klams-monitor → POST /memory/events`. New "Phase 3 deltas (sprint 003)" subsection listing FR themes with SC pointers.
- [ ] T054 [P] Update [docs/setup.md](docs/setup.md): `just install-systemd` walkthrough, scanner config TOML block, monitor config TOML block, the `klams` system user note, `journalctl -u klams-service` pointer.
- [ ] T055 [P] Update [docs/usage.md](docs/usage.md): `GET /memory/policy` example, the new `path` field on write responses (with a callout that existing clients are unaffected), pointer to the handoff document for ansible integrators, scanner one-shot recipe.
- [ ] T056 [P] Update [README.md](README.md): "Sprint 003 quick reference" subsection inside "Running the MVP" with the literal `just --list` output (adds the four new recipes), link to [specs/003-non-agentic-writes/quickstart.md](specs/003-non-agentic-writes/quickstart.md).
- [ ] T057 Walk [quickstart.md](quickstart.md) §1–§8 against `kubs0` (or the test stack as a proxy for steps that don't need real systemd). Fill in the "Phase 3 walkthrough" table at the end of [spec.md](spec.md) with PASS/FAIL + evidence per row, mirroring sprint 002's table structure.
- [ ] T058 Final `just gate` from a clean tree; resolve any new clippy/test fallout. Re-run the sprint-001 and sprint-002 integration suites (`us1_*`, `us2_*`, `us4_*`, `us5_*`, `perf_smoke --ignored`) and confirm zero regressions per FR-023.
- [ ] T059 Ship the handoff: `cp -r specs/003-non-agentic-writes/handoff/ /home/ken/ansible-k/specs/klams-integration/`. Ken commits the import in the ansible-k repo (out of this sprint's commit scope). Confirm the directory is in place; mark SC-006 testable.

---

## Dependencies

- **Phase 1 (Setup)** has no dependencies; T001 blocks everything else; T002/T003 are parallel within Phase 1.
- **Phase 2 (Foundational)** depends on Phase 1. Within Phase 2: T004 is standalone; T005/T006 parallel; T007 depends on T005; T008/T009 parallel after T005; T010 depends on T008; T010a and T010b are both parallel with the rest of Phase 2 (independent endpoints, separate test files).
- **Phase 3 (US5)** depends on Phase 2 fully closed (needs `PolicyTable` and `path` field).
- **Phase 4 (US1)** depends on Phase 2 (needs dispatcher refactor T007) and may run in parallel with Phase 3.
- **Phase 5 (US3)** depends on Phase 2 + on the test-docker-stack helpers in `klams-service/tests/common.rs` (sprint 002).
- **Phase 6 (US2)** depends on Phase 1's binary scaffold (T001/T002) and Phase 2's `path` field landing (so the integration test can assert the response shape). Parallel with Phase 5.
- **Phase 7 (US4)** depends on Phase 6 (`klams-scanner` exists) and Phase 5 (`klams-monitor` exists) — both binaries must build before the unit files can `ExecStart` them.
- **Phase 8 (US6)** depends on the contracts being final (Phase 2 closed: write-response shape and policy endpoint shape). Parallel with Phases 5/6/7 from a tasks-graph standpoint, but the contract tests in T048 use the same `tests/` directory style as US3, so it's natural to land alongside.
- **Phase 9 (Polish)** depends on all story phases.

## Parallel execution examples

- **Within Phase 2**: T005, T006, T008 can all be authored in parallel (different files); T010 in parallel with T008/T009 (different test files).
- **Across phases after Phase 2 closes**: Phase 3, Phase 4, Phase 5, Phase 6, and Phase 8 can all proceed in parallel (different crates/files); Phase 7 waits for Phase 5+6 binaries to exist.
- **Within Phase 6**: T030/T031/T032 (unit tests) all in parallel; T034/T035/T036 (implementation files) all in parallel.
- **Within Phase 9**: T053/T054/T055/T056 all in parallel (different doc files).

## Implementation strategy

**MVP scope** (what to ship if forced to stop early):

- Phase 1 (Setup) + Phase 2 (Foundational) + Phase 3 (US5) + Phase 8 (US6) = the public API contract closes, the handoff document is shipped, the ansible-k owner can begin in parallel. Without US1/US2/US3/US4, klams' production behavior is unchanged; the surface is documented.

**Incremental delivery** (after MVP):

- US1 next (smallest delta; one validator change + one canonical-hash tweak) — unlocks the ansible-k owner to start sending real writes.
- US3 next (server validators + monitor binary) — closes the events story; SC-003 testable.
- US2 next (scanner binary) — biggest single deliverable; closes the knowledge story; SC-002 testable.
- US4 last (systemd switchover) — depends on the two binaries existing; ships the prod-lifecycle change; SC-004 testable.

**Final gates** (Phase 9):

- `just gate` green; sprint-001 and sprint-002 integration suites green (FR-022, FR-023, SC-007).
- Walkthrough table filled in [spec.md](spec.md) with PASS rows.
- Handoff copied to `/home/ken/ansible-k/specs/klams-integration/` (SC-006 prereq).
