# Implementation Plan: Non-Agentic Writes, Integrations, and the Systemd Switchover

**Branch**: `003-non-agentic-writes` | **Date**: 2026-05-18 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/003-non-agentic-writes/spec.md`

## Summary

Sprint 003 wires klams into the homelab's existing automation surface
and flips production to systemd. The shipped behavior:

1. The klams API contract grows two small but pointed extensions —
   a `path`/`dissent_id` field on every write response and a read-only
   `GET /memory/policy` projection of the source-trust table —
   so non-agentic writers get observable, debuggable behavior.
2. A new `klams-scanner` binary (separate target in the existing
   workspace) walks `~/src` and `~/obsidian`, chunks changed files,
   and indexes them via the existing `POST /memory/knowledge/index`
   path. Cursor lives in a small sqlite file under `$XDG_STATE_HOME`
   to keep Postgres' schema unchanged this sprint.
3. A new `klams-monitor` binary tails `systemctl is-active` for a
   configured set of units and posts `category=Service` events.
4. `klams-service.service`, `klams-scanner.timer/.service`, and
   `klams-monitor.service` ship under `deploy/`; a `just install-systemd`
   recipe wires them into `kubs0` idempotently. `just run` survives as
   the foreground debugger.
5. The handoff document for ansible-k is staged under
   `specs/003-non-agentic-writes/handoff/` during the sprint and
   `cp -r`'d into `/home/ken/ansible-k/specs/klams-integration/` as the
   final ship step — keeping the authoring loop inside this repo's
   spec-driven cycle while honoring the user's preferred destination.

Approach: keep the Phase 1/2 binary and crate boundaries unchanged.
Add two thin binaries to the existing workspace, one new HTTP read
endpoint, and one new response field on existing write endpoints.
Everything else is deploy/docs/integration-test work.

## Technical Context

**Language/Version**: Rust (workspace MSRV pinned in `rust-toolchain.toml`; matches Phase 1/2 — stable 1.81+).
**Primary Dependencies**:

- Existing: `tokio`, `axum`, `sqlx`, `qdrant-client`, `reqwest`, `serde`, `tracing`, `prometheus`, `clap`.
- New (scanner): [`ignore`](https://crates.io/crates/ignore) (gitignore-aware walk; the same library `ripgrep` uses), [`rusqlite`](https://crates.io/crates/rusqlite) with `bundled` feature for the local cursor store, `sha2` (already in tree) for content hashing.
- New (monitor): `tokio::process::Command` calling `systemctl`; no extra crate.
- No new crates in `klams-service`.

**Storage**:

- Postgres (existing `klams` DB): one additive migration `0003_events_task_idx.sql` that adds `CREATE INDEX IF NOT EXISTS events_task_id_created_at_idx ON events (task_id, created_at)` per FR-010. No new tables.
- Qdrant: unchanged.
- Scanner cursor: local sqlite at `${XDG_STATE_HOME:-$HOME/.local/state}/klams/scanner.sqlite`, schema = single table `(absolute_path TEXT PRIMARY KEY, content_hash TEXT NOT NULL, mtime_ns INTEGER NOT NULL, last_indexed_at INTEGER NOT NULL)`.

**Testing**:

- `cargo test --workspace` for unit + contract + integration.
- New integration tests added under `crates/klams-service/tests/`: `us3a_policy_endpoint.rs`, `us3b_scanner_e2e.rs` (`#[ignore]`-gated, runs against the existing test docker stack), `us3c_monitor.rs` (mocked `systemctl` via a fake binary on `PATH`).
- New unit tests in `klams-scanner` for the chunk-and-diff logic; in `klams-monitor` for the state-diff/post logic.

**Target Platform**: Linux x86_64 on `kubs0`. Systemd >= 252. Dev box (`kai`) continues to use `just run`.

**Project Type**: Rust workspace (single project, web-service shape) with the existing six member crates plus two new binary targets in their own crates.

**Performance Goals**:

- Scanner: 1k files / 60s on a warm cache (incremental scans). Cold full scan of `~/src` + `~/obsidian` under 15 minutes.
- Monitor: poll budget <= 1s every 15s (8 units x ~100ms `systemctl is-active`).
- `GET /memory/policy`: < 5ms p99 (in-memory struct serialization).

**Constraints**:

- The systemd unit MUST run as a dedicated `klams` system user with no shell login; install recipe creates the user idempotently.
- Binary rotation MUST be atomic (`mv` only — no `cp` mid-flight). Implementation uses a temp path + `rename(2)` per FR-014.
- All new HTTP endpoints reuse the existing bearer-token middleware; no new auth surface this sprint.

**Scale/Scope**:

- Estimated 20-30 source files added (two new crates plus existing-crate edits).
- ~1500 LOC including tests and deploy templates.
- 6 user stories, 23 FRs, 7 SCs from the spec.

## Constitution Check

Gates from `.specify/memory/constitution.md` (v1.0.0):

| Principle | Gate | Status |
|-----------|------|--------|
| I. SDD | Spec exists at `specs/003-non-agentic-writes/spec.md`, all FRs traceable to user stories. | PASS |
| II. TDD | Each new binary lands behind a failing integration test first (`us3a/us3b/us3c`); contract test for `/memory/policy` precedes handler. | PASS |
| III. Code Standards Gate | `just gate` (sprint 002) is the constitution gate; CI already invokes it. No new lint exceptions requested. | PASS |
| IV. Documentation | `docs/architecture.md` (scanner + monitor + systemd topology), `docs/setup.md` (`just install-systemd` recipe + scanner config), `docs/usage.md` (`GET /memory/policy`, write-response `path` field), `README.md` (Sprint 003 quick reference) — all in scope. | PASS (planned) |
| V. Quality & Observability | New Prometheus metrics per FR-007 + FR-017; `tracing` spans on scanner walks and monitor polls; structured `klams_writes_total{type, source, path}` covers the new dimension. | PASS |
| VI. Simplicity | Two thin binaries reusing existing HTTP endpoints; one new GET; one response-field addition. No new abstractions, no new storage backends, no premature config layers. | PASS |

**Complexity Tracking**: none — no violations require justification.

Re-check after Phase 1 design: pending (see "Post-Design Re-Check" below).

## Project Structure

### Documentation (this feature)

```text
specs/003-non-agentic-writes/
|-- plan.md              # This file
|-- research.md          # Phase 0 — research decisions and rationale
|-- data-model.md        # Phase 1 — entities, indexes, cursor schema
|-- quickstart.md        # Phase 1 — walkthrough an operator can run on kubs0
|-- contracts/
|   |-- memory_policy.md       # GET /memory/policy contract
|   |-- write_response.md      # Updated write-endpoint response shape
|   `-- handoff_index.md       # Handoff-doc structure contract
|-- handoff/             # Staged ansible-k handoff (cp -r'd at ship time)
|   |-- README.md
|   |-- spec.md
|   |-- api-contract.md
|   `-- examples/
|       `-- post-userfact.sh
|-- checklists/
|   `-- requirements.md  # Spec quality checklist (already created)
`-- tasks.md             # Phase 2 output (NOT created by /speckit.plan)
```

### Source Code (repository root)

```text
crates/
|-- klams-types/                 # +ServiceEvent / ExecutionTraceEvent payload validators
|-- klams-core/                  # +PolicyTable struct + JSON projection
|-- klams-store/                 # no schema changes beyond +1 migration
|-- klams-api/                   # +GET /memory/policy handler, +path field on write responses
|-- klams-service/               # +tests/us3a_policy_endpoint.rs, +tests/us3b_scanner_e2e.rs, +tests/us3c_monitor.rs
|-- klams-client/                # +PolicyClient method (read-only), +path field surfaced on Write* results
|-- klams-scanner/               # NEW — binary: scanner CLI + lib (walk, chunk, diff, post)
|   |-- src/
|   |   |-- main.rs              # CLI entry, clap config
|   |   |-- lib.rs               # public API for tests
|   |   |-- walk.rs              # ignore-crate wrapper, .klamsignore handling
|   |   |-- chunk.rs             # text chunking + sha256
|   |   |-- cursor.rs            # rusqlite cursor store
|   |   `-- publish.rs           # reqwest client -> /memory/knowledge/index
|   `-- tests/
|       `-- walk_diff.rs
`-- klams-monitor/               # NEW — binary: systemctl poller
    |-- src/
    |   |-- main.rs              # CLI entry, clap config
    |   |-- lib.rs
    |   |-- poll.rs              # systemctl is-active wrapper
    |   |-- state.rs             # in-memory previous-state cache
    |   `-- publish.rs           # reqwest client -> /memory/events
    `-- tests/
        `-- state_diff.rs

deploy/
|-- klams-service.service        # existing path is `deploy/config/...`; this sprint lifts the .service to deploy/
|-- klams-scanner.service
|-- klams-scanner.timer
|-- klams-monitor.service
`-- install-systemd.sh           # called by `just install-systemd`

migrations/
`-- 0003_events_task_idx.sql

justfile                         # +install-systemd, +scanner-once, +monitor-once recipes

docs/
|-- architecture.md              # +scanner + monitor + systemd topology
|-- setup.md                     # +install-systemd, +scanner config block
`-- usage.md                     # +GET /memory/policy, +path field, +scanner config
```

**Structure Decision**: keep the workspace shape established in Phase 0 and unchanged through Phase 2. Add two new binary-only crates (`klams-scanner`, `klams-monitor`) rather than embedding the scanner/monitor as tokio tasks inside `klams-service`. Rationale and the alternatives considered are recorded in [research.md](research.md).

## Phase 0 — Research

See [research.md](research.md) for the resolved decisions. Topics:

1. Scanner: separate binary vs in-process tokio task.
2. Cursor store: sqlite vs Postgres table vs flat-file `.json`.
3. Walk implementation: `walkdir` vs `ignore` vs hand-rolled.
4. Chunking strategy: lines-with-overlap vs markdown-headings vs token-bounded.
5. Service monitor: poll `systemctl` vs subscribe to dbus.
6. `path` field placement on write responses.
7. `GET /memory/policy` shape.
8. Binary rotation strategy.
9. Handoff document layout (mirrors speckit cycle vs ad-hoc).

All NEEDS CLARIFICATION items from spec.md are resolved — none of the
FR or SC entries contained `[NEEDS CLARIFICATION]` markers; the
research decisions cement implementation choices that the spec
deliberately left open.

## Phase 1 — Design & Contracts

Outputs (alongside this file):

- [data-model.md](data-model.md) — entity-by-entity field layout: `AnsibleFactWrite` (no DB shape change — distinguished by `task_id`), `ScannedKnowledgeChunk` payload, `ServiceEvent`/`ExecutionTraceEvent` payload constraints, `PolicyTable` struct, scanner cursor sqlite schema, migration `0003_events_task_idx.sql`.
- [contracts/memory_policy.md](contracts/memory_policy.md) — request/response for the new read endpoint, including the contract-test row that compares the JSON against the in-memory struct.
- [contracts/write_response.md](contracts/write_response.md) — the additive `path` + optional `dissent_id` fields on `POST /memory/facts`, `POST /memory/events`, `POST /memory/knowledge/index`, and the back-compat note (existing fields unchanged; clients that ignore unknown fields keep working).
- [contracts/handoff_index.md](contracts/handoff_index.md) — the required structure of the handoff directory (README → spec → api-contract → examples), the pinned-version header, and the drift-detection convention.
- [quickstart.md](quickstart.md) — operator-facing walkthrough: build → run migration → `just install-systemd` → `journalctl -u klams-service` smoke → scanner timer first-fire → monitor service-restart test → `curl GET /memory/policy` → `curl POST /memory/facts` with a `source=Task` payload showing `path: "canonical"` → `cp -r specs/003-non-agentic-writes/handoff/ /home/ken/ansible-k/specs/klams-integration/` → re-run sprint 002 walkthrough to confirm zero regressions.

### Agent context update

The reference between the `<!-- SPECKIT START -->` and `<!-- SPECKIT END -->` markers in `.github/copilot-instructions.md` will be updated to point at `specs/003-non-agentic-writes/plan.md` (currently at sprint-002).

## Post-Design Re-Check (Constitution)

Verified after Phase 1 artifacts landed. Outcome: **PASS** on all six
principles. The design is strictly additive — no existing endpoint
contract changes (the new write-response fields are optional and
back-compat per [contracts/write_response.md](contracts/write_response.md)),
no schema reshape (one CONCURRENTLY-safe index added), no new auth
surface, no new abstractions inside `klams-service`. The two new
binaries are siblings of the existing service binary in the same
workspace and reuse the existing HTTP client crate (`klams-client`)
plus the existing bearer-token middleware. No complexity-tracking
entries required.

## Complexity Tracking

*No violations to justify.*

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|--------------------------------------|
| _(none)_  | _(none)_   | _(none)_                             |
