# Implementation Plan: Maintenance, Backups, and Ops

**Branch**: `006-maintenance-and-backups` | **Date**: 2026-05-23 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/sprints/006-maintenance-and-backups/spec.md`

## Summary

Add an in-process backup scheduler to the existing `klams-service` daemon
that produces a nightly Postgres dump and a nightly Qdrant snapshot,
lands both atomically in a configured local filesystem path, gates
non-critical write endpoints behind a `maintenance_mode` flag during the
run, and invokes a single configurable external executable
(`status_hook`) at the `started` / `finished` / `failed` lifecycle
points with a versioned JSON report on stdin. Ship a documented +
once-exercised restore procedure and a Grafana dashboard JSON over the
new + existing Prometheus series, plus a handoff document under
`~/ansible-k/specs/klams-integration/klams-grafana.md` so the ansible-k
project (which owns the kubsdb Grafana instance) can provision the
dashboard, datasource binding, and alerts. Cloud sync is explicitly
out of scope — the contract ends at the file in `backup_dir`.
Grafana operationalization (provisioning, alert wiring, notifier
routing) is explicitly out of scope for klams — the contract ends at
the `klams.json` file and the handoff document.

The conversation tweaks vs. the original Phase 5 sketch:

1. **Status hook fires at backup *start*** in addition to finish — so
   kpidash can show in-flight runs, not only "completed N minutes ago"
   widgets.
2. **`window_start_utc` (not `window_start`)** — UTC-only config string
   in `HH:MM`, so DST shifts can never silently change when the backup
   runs.

A Day-0 sizing micro-task (`T0`) scales the existing test fixtures to
realistic homelab volumes (~10k facts, ~50k events, ~20k knowledge
chunks) and times `pg_dump` + qdrant snapshot + NAS copy. This both
guides retention defaults and unblocks the deferred sprint-005
benchmarks (T055/T056 — SC-001/SC-003/SC-004).

## Technical Context

**Language/Version**: Rust 1.83 (workspace pinned in `rust-toolchain.toml`)  
**Primary Dependencies**: existing — `tokio` 1.x, `axum` 0.7, `sqlx` 0.8 (Postgres), `qdrant-client` 1.12, `reqwest` 0.12, `tracing` 0.1, `prometheus` 0.13, `serde` 1, `serde_json` 1, `chrono` 0.4 (with `serde`), `ulid` 1; new — `tokio-cron-scheduler` 0.13 (or a hand-rolled `tokio::time::interval` if scheduler dep adds disproportionate weight; see research.md R-002)  
**Storage**: Postgres 16 (snapshot source), Qdrant v1.12.4 (snapshot source), local filesystem at `[backup] backup_dir` (snapshot sink — typically a NAS mount but klams MUST NOT care)  
**Testing**: `cargo test --workspace` (unit + integration), `tests/docker-compose.test.yml` (Postgres + Qdrant + TEI fixture), `just gate` as the CI mirror (`cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`)  
**Target Platform**: Linux server (kubs0 systemd unit), with the existing Windows / WSL viewport unaffected (viewport is a read-only consumer of `/memory/*` — it doesn't write, so maintenance mode is transparent to it)  
**Project Type**: Cargo workspace, web service (klams-service) + multiple supporting crates; the backup feature lives primarily in `klams-service` with thin extensions to `klams-types` (config), `klams-store` (snapshot helpers wrapping `pg_dump` / qdrant snapshot API), and `klams-api` (maintenance-mode middleware)  
**Performance Goals**: Backup completes in < 5 minutes for the homelab data volume (validated by T0 sizing run); maintenance-mode middleware adds < 100µs to non-critical write paths; status_hook invocation overhead < 50ms end-to-end excluding the hook process itself  
**Constraints**: Hook executable runs synchronously w.r.t. the lifecycle event but with a `status_hook_timeout` (default `10s`); a misbehaving hook MUST NOT affect backup outcome; `maintenance_mode` MUST NOT gate reads; `enabled = false` (default) means zero runtime cost  
**Scale/Scope**: Single klams instance, single Postgres, single Qdrant — no clustering, no multi-region; one backup window per day; retention default 14 daily + 4 weekly per artifact kind  

## Constitution Check

*Re-checked after Phase 1 design — both passes record below.*

| Principle | Initial gate (pre-design) | Post-Phase-1 gate | Notes |
|-----------|---------------------------|-------------------|-------|
| I. SDD | PASS | PASS | spec.md captured before any code; this plan + research/data-model/quickstart/contracts complete the SDD artifact set before tasks are emitted |
| II. TDD | PASS | PASS | Each FR-NNN traces to an acceptance test in quickstart.md and a unit/integration test slot in the future tasks.md; status-hook JSON schema has a contract test before any emitter code |
| III. Code Standards | PASS | PASS | `just gate` is the unchanged exit gate; no new lints relaxed; no new clippy allows |
| IV. Documentation | PASS | PASS | `docs/setup.md` (restore section), `docs/usage.md` (maintenance window + hook config + new just recipes), `docs/architecture.md` (backup component box) all on the deliverables list (see Phase 1 → Documentation) |
| V. Quality & Observability | PASS | PASS | Five new Prometheus series + structured `tracing` spans for every backup phase + journal capture of hook stdout/stderr; `/healthz` extension (maintenance_mode bool) is additive and documented |
| VI. Simplicity & Intentional Design | PASS | PASS | One external integration point (status_hook, exec + stdin JSON), no message brokers; in-memory `BackupRun` instead of a new Postgres table; cloud sync deferred so we don't grow an abstraction we wouldn't use today; scheduler choice deferred to research.md R-002 to avoid premature dependency growth |

No principle violations require justification in **Complexity Tracking**.

## Project Structure

### Documentation (this feature)

```text
sprints/006-maintenance-and-backups/
├── spec.md              # /speckit.specify output (already drafted in this run)
├── plan.md              # this file
├── research.md          # Phase 0 output (this run)
├── data-model.md        # Phase 1 output (this run)
├── quickstart.md        # Phase 1 output (this run)
├── contracts/
│   └── backup-status-hook.schema.json   # JSON Schema 2020-12 for the hook stdin payload
└── tasks.md             # Phase 2 output (NOT created here — /speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── klams-types/
│   └── src/config.rs            # +BackupConfig { enabled, backup_dir, window_start_utc,
│                                #                 daily_count, weekly_count, same_day_strategy,
│                                #                 status_hook, status_hook_timeout }
├── klams-store/
│   └── src/backup/              # NEW module
│       ├── mod.rs               # facade
│       ├── postgres.rs          # pg_dump invocation (Command, custom format -Fc)
│       ├── qdrant.rs            # qdrant snapshot REST + download
│       └── retention.rs         # daily/weekly pruning, .partial recovery
├── klams-api/
│   └── src/middleware/
│       └── maintenance.rs       # NEW: 503 + Retry-After on non-critical writes
├── klams-service/
│   └── src/backup/              # NEW module
│       ├── mod.rs               # wiring: BackupTask, MaintenanceState
│       ├── scheduler.rs         # UTC-time-based one-shot-per-day trigger
│       ├── lifecycle.rs         # BackupRun state machine: started → running → finished/failed
│       ├── hook.rs              # status_hook executor (stdin pipe, timeout, signal handling)
│       ├── restore.rs           # NEW: restore::run_from(date, force) — drives the Phase 4 restore path
│       └── metrics.rs           # register/export the 5 new Prometheus series
│   # plus three new binary subcommand handlers on the existing
│   # `klams-service` bin (CLI shape, not a new crate):
│   #   --validate-backup-config   (loads klams.toml, runs BackupConfig::validate, exits 0/2)
│   #   --run-backup-now           (drives backup::run_once directly, bypasses scheduler)
│   #   --restore-from <date> [--force]  (drives restore::run_from)
└── (other crates unchanged)

tests/
├── integration/
│   ├── backup_pg_dump.rs        # spawns a real pg_dump against the test compose Postgres
│   ├── backup_qdrant_snapshot.rs# real qdrant snapshot API call
│   ├── backup_status_hook.rs    # invokes a known-good shell script + a known-bad one
│   ├── backup_retention.rs      # creates dated fixture files, asserts pruning rules
│   ├── maintenance_middleware.rs# axum test: 503 on writes, 200 on reads, dissent passthrough
│   └── restore_roundtrip.rs     # SC-002: snapshot → restore → counts match
└── fixtures/
    └── backup/
        └── sample-hook.sh       # writes stdin JSON to a temp file, exits 0

deploy/
├── grafana/
│   └── klams.json               # NEW: dashboard JSON
└── systemd/                     # existing; verified unchanged

docs/
├── setup.md                     # +"Restore from snapshot" section
├── usage.md                     # +backup config, new just recipes, status_hook contract pointer
└── architecture.md              # +Backup/Maintenance component note + diagram tweak

justfile                         # +backup-once, +restore-from <date>, +backup-validate-config
```

**Structure Decision**: Reuse the existing 8-crate workspace. The backup
feature is service-scoped runtime behavior, so its glue lives in
`klams-service`; the snapshot mechanics (which shell out to `pg_dump`
and call the qdrant snapshot REST API) live in `klams-store` next to
the other Postgres + Qdrant code; the maintenance-mode HTTP middleware
lives in `klams-api`; config types live in `klams-types` next to
the other `*Config` structs. No new crate is needed.

## Complexity Tracking

*No constitution violations to justify; section intentionally empty.*

## Phase 0: Research summary

The full Phase 0 output is in [research.md](./research.md). Decisions
that shape this plan:

- **R-001 pg_dump invocation strategy** → `tokio::process::Command`,
  custom format (`-Fc`), credentials via the existing `klams.toml`
  Postgres block exported as `PGPASSWORD` env, write to
  `<backup_dir>/postgres-<UTC-date>.dump.partial` then atomic rename.
- **R-002 scheduler choice** → hand-rolled `tokio::time::sleep_until`
  loop computing the next `window_start_utc` instant; rejected
  `tokio-cron-scheduler` as overweight for a single daily trigger.
- **R-003 qdrant snapshot mechanism** → `POST /collections/{name}/snapshots`
  via `qdrant-client`'s low-level REST shim (no native typed wrapper
  in 1.12), then `GET /snapshots/{name}` streaming download to disk.
- **R-004 status_hook process model** → `tokio::process::Command` with
  piped stdin, no env passthrough beyond `KLAMS_BACKUP_RUN_ID`,
  `KLAMS_BACKUP_EVENT`; stdout/stderr captured to a 4 KiB ring; timeout
  via `tokio::time::timeout` → SIGTERM → 2s grace → SIGKILL.
- **R-005 maintenance-mode middleware shape** → axum `from_fn` layer
  reading an `Arc<AtomicBool>` (no contention, single check per
  request); per-route opt-in for "critical" verbs via a marker
  extension.
- **R-006 retention pruning** → list `backup_dir` for `<kind>-*.dump`
  / `<kind>-*.tar.zst`, parse dates from filename only (mtime not
  authoritative — NAS clock skew), keep newest N daily + N Sunday
  weekly, delete the rest. Runs only after a successful new artifact.
- **R-007 Grafana panel set** → mirror the existing kubs0 panels for
  service/queue + add five backup-specific panels; dashboard JSON
  hand-authored at `deploy/grafana/klams.json`. Operational
  provisioning + alert wiring delegated to ansible-k via
  `~/ansible-k/specs/klams-integration/klams-grafana.md` (klams ships
  the JSON and the contract; ansible-k pulls a pinned klams tag and
  installs).
- **R-008 restore validation** → SC-002 done as an integration test that
  writes a known fixture, snapshots, restores into a fresh compose
  stack, and diffs counts + a canonical 10-row sample.
- **R-009 Day-0 sizing** → `tests/fixtures/scale_loader.rs` extends
  sprint-005's seed scripts to push ~10k facts, ~50k events, ~20k
  knowledge chunks; `just backup-size` runs once-off and prints
  per-artifact bytes + seconds. Output informs retention defaults.

## Phase 1: Design & Contracts summary

The full outputs are in [data-model.md](./data-model.md),
[quickstart.md](./quickstart.md), and
[contracts/](./contracts/). Highlights:

- **Data model** is in-memory only: `BackupRun` and `BackupArtifact`
  structs in `klams-service::backup::lifecycle`; `MaintenanceState`
  with an `Arc<AtomicBool>` plus an `Arc<RwLock<Option<BackupRunSnapshot>>>`
  for `/healthz` exposure.
- **Wire contract** for the status hook: a single JSON Schema 2020-12
  document at `contracts/backup-status-hook.schema.json` that pins
  `schema_version`, `run_id`, `event ∈ {started, finished, failed}`,
  `started_at`, `ended_at?`, `duration_ms?`, `artifacts[] = {kind,
  path, bytes, duration_ms, ok, error?}`, `ok`, `error?`. A contract
  test under `tests/integration/backup_status_hook.rs` validates every
  emitter call against the schema (using `jsonschema` crate dev-dep).
- **HTTP error envelope** for the 503: matches the existing
  `/memory/context` outage shape from sprint 005 FR-011 — single
  `error` field plus per-error extension fields. Adds
  `retry_after_seconds` next to the `Retry-After` header so JSON
  clients don't need to parse headers.
- **Config block** (TOML, fully in `klams.toml`):
  ```toml
  [backup]
  enabled = false
  backup_dir = "/mnt/gratch/klams"
  window_start_utc = "10:00"        # 24h UTC, dodges DST
  daily_count = 14
  weekly_count = 4
  same_day_strategy = "suffix"      # "suffix" | "overwrite"
  status_hook = "/usr/local/bin/klams-backup-status"   # optional; unset = no hook
  status_hook_timeout = "10s"
  ```
- **Agent context update**: `.github/copilot-instructions.md` block
  between `<!-- SPECKIT START -->` and `<!-- SPECKIT END -->` now
  points at this plan file (`sprints/006-maintenance-and-backups/plan.md`).
- **Documentation deltas** (committed alongside the code, not as
  follow-up): `docs/setup.md` gains "Restore from snapshot";
  `docs/usage.md` gains a `[backup]` config example, the new `just`
  recipes (`backup-once`, `restore-from`, `backup-validate-config`),
  and a pointer to the status_hook contract; `docs/architecture.md`
  gains a one-paragraph "Backup & maintenance" subsection and a
  ASCII-diagram box next to `klams-service`.

## Key rules carried into Phase 2 (tasks.md)

- **T0 first**: sizing micro-task that also produces the >=1k-fact
  fixture, unblocking deferred sprint-005 benchmarks T055/T056.
- **Sprint task ordering**: T0 sizing → pg_dump module → qdrant
  snapshot module → retention → backup orchestrator + scheduler →
  maintenance middleware → status_hook executor → metrics wiring →
  `just` recipes (`backup-once`, `restore-from`, `backup-validate-config`)
  → Grafana JSON + manual import smoke test → ansible-k handoff doc
  at `~/ansible-k/specs/klams-integration/klams-grafana.md` →
  restore documentation + exercised end-to-end → doc updates →
  `just gate`.
- **No code merges before** the contract test for the status_hook JSON
  Schema is green (TDD per Constitution II).
- **Cloud sync stays absent**: do not add a `[backup.cloud]` block,
  an `s3` feature flag, a `rclone` dependency, or any plumbing
  toward future offsite sync. The boundary is the file in
  `backup_dir`.

## Stop and report

- **Branch**: `006-maintenance-and-backups`
- **Plan**: [sprints/006-maintenance-and-backups/plan.md](./plan.md)
- **Artifacts generated this run**:
  - [spec.md](./spec.md)
  - [plan.md](./plan.md) (this file)
  - [research.md](./research.md)
  - [data-model.md](./data-model.md)
  - [quickstart.md](./quickstart.md)
  - [contracts/backup-status-hook.schema.json](./contracts/backup-status-hook.schema.json)
- **Next command**: `/speckit.tasks` to generate `tasks.md`.
