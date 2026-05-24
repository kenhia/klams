# Phase 0 Research — Maintenance, Backups, and Ops

Each section resolves a `NEEDS CLARIFICATION` or dependency choice
identified while drafting [plan.md](./plan.md).

## R-001 — `pg_dump` invocation strategy

**Decision**: Shell out to the system `pg_dump` binary via
`tokio::process::Command`, custom format (`-Fc`), credentials sourced
from the same `klams.toml` Postgres block already used by `sqlx` at
runtime, exported as `PGPASSWORD` env on the child only. Output goes
to `<backup_dir>/postgres-<UTC-date>.dump.partial` and is atomically
renamed to `postgres-<UTC-date>.dump` only after a clean exit.

**Rationale**:

- `pg_dump -Fc` is the documented input to `pg_restore` and supports
  selective restore, compression, and parallel restore — strictly more
  capable than plain SQL.
- Custom format already compresses (`zlib`) so we don't need to layer
  `zstd` around it.
- Shelling out avoids embedding a `pg_dump`-equivalent Rust library
  (no mature option exists) and matches operator muscle memory: any
  Postgres admin knows `pg_dump` / `pg_restore`.
- Atomic rename gives the retention pruner a safe "live file" /
  "in-progress" distinction it can rely on across crashes.

**Alternatives considered**:

- `pg_basebackup` — physical replication, larger files, harder to
  restore selectively; rejected for a snapshot use case.
- Logical replication slot + `wal2json` consumer — overkill; would
  reopen the PITR question this sprint explicitly defers.
- `sqlx`-based row-by-row export — no SQL-syntax guarantees against
  Postgres extension types; rejected.

## R-002 — Scheduler choice

**Decision**: Hand-rolled `tokio::time::sleep_until` loop that
computes the next instant matching `window_start_utc` (HH:MM) and
sleeps until then. No external scheduler crate.

**Rationale**:

- One trigger per day with a single human-readable HH:MM config —
  zero advantage from cron syntax.
- `tokio-cron-scheduler` pulls in `cron` + `uuid` + its own job
  registry; adds dependency surface area for no functional gain.
- Hand-rolled is ~30 lines, fully testable with `tokio::time::pause()`,
  and aligns with Constitution VI (Simplicity).

**Alternatives considered**:

- `tokio-cron-scheduler` — rejected as overweight.
- `cron` crate + manual loop — adds dependency for the parsing of a
  string we never expose; rejected.
- External `systemd-timer` — would move the scheduling out of klams
  and into the unit file. Rejected because the maintenance-mode flag
  must flip from inside the running process (a systemd-triggered
  one-shot binary couldn't do that without a separate IPC channel).

## R-003 — Qdrant snapshot mechanism

**Decision**: Take the snapshot via Qdrant's native REST API
(`POST /collections/{name}/snapshots`), then download the resulting
file via `GET /collections/{name}/snapshots/{snapshot_name}` as a
streaming `reqwest` response to
`<backup_dir>/qdrant-<UTC-date>.snapshot.partial`, atomic-rename on
completion. Optionally drop the in-qdrant copy after successful
download (configurable, default = drop).

**Rationale**:

- Native snapshot API guarantees a consistent point-in-time copy of
  the collection's vectors + payloads + HNSW index, which is what
  `qdrant-client`'s typed methods do not directly expose for
  download in v1.12.
- Streaming download keeps memory bounded regardless of collection
  size.
- Dropping the in-qdrant copy after download avoids unbounded growth
  in qdrant's own storage volume.

**Alternatives considered**:

- Reading raw qdrant storage directories — undocumented, would break
  on qdrant upgrades; rejected.
- Re-creating the collection by paging through points and
  re-embedding — slow, lossy w.r.t. quantization, and the qdrant
  snapshot is the documented mechanism; rejected.

## R-004 — `status_hook` process model

**Decision**: `tokio::process::Command` with stdin set to a pipe;
JSON payload written and pipe closed; spawn under
`tokio::time::timeout(status_hook_timeout, ...)`; on timeout, send
SIGTERM, wait 2s, then SIGKILL. Pass two env vars (`KLAMS_BACKUP_RUN_ID`,
`KLAMS_BACKUP_EVENT`) for fast triage scripts that don't want to
parse JSON; everything else is in the stdin payload. Capture stdout
and stderr to per-stream 4 KiB ring buffers and emit a single
`tracing` event per hook invocation.

**Rationale**:

- exec-with-JSON-on-stdin is the most portable IPC: works for shell
  scripts, Python one-liners, Go binaries, anything.
- Two env vars give the simplest possible "is this a started or a
  finished" check (`[ "$KLAMS_BACKUP_EVENT" = "started" ]`) without
  forcing every shim to add a JSON parser.
- 4 KiB ring cap keeps the journal small for chatty hooks.
- SIGTERM → grace → SIGKILL is the standard Unix shutdown pattern;
  refusing to honor SIGKILL is the kernel's job, not ours.

**Alternatives considered**:

- Unix domain socket — more setup, no real benefit; rejected.
- HTTP POST to a configurable webhook — would require klams to grow
  an HTTP client for outbound webhooks, would dictate the receiver's
  shape (HTTP server), and breaks the simple-shim case; rejected.
- Single env-var-only contract (no stdin JSON) — couldn't pass
  per-artifact details; rejected.

## R-005 — Maintenance-mode middleware shape

**Decision**: An axum `from_fn` middleware reading an
`Arc<AtomicBool>`. Routes opt into "critical" status via a marker
extension applied at router build time (the dissent
promote/discard handlers register themselves as critical). The
middleware short-circuits non-critical, non-GET requests with
`503 Service Unavailable + Retry-After + JSON envelope` when the
flag is set; reads and critical writes pass through unchanged.

**Rationale**:

- `AtomicBool::load(Ordering::Relaxed)` is sub-nanosecond per
  request — meets the < 100µs constraint trivially.
- The marker-extension approach keeps the criticality decision next
  to the route definition, where it belongs (not in a separate
  config file).
- `from_fn` is the documented axum 0.7 extension point; no custom
  Service implementation needed.

**Alternatives considered**:

- Per-handler `if state.maintenance_mode() { return 503 }` checks —
  duplicative, easy to forget on new endpoints; rejected.
- A global `RwLock<bool>` — fine but `AtomicBool` is the clearer
  expression of "this is a flag, not a structure"; rejected.
- A token-bucket style gradual shed — overengineered for the binary
  on/off behavior the spec requires; rejected.

## R-006 — Retention pruning

**Decision**: List `backup_dir`, filter by the `<kind>-` prefix,
parse the date from the filename (canonical format
`<kind>-YYYY-MM-DD[-N].{dump,snapshot}`), keep the newest
`daily_count` distinct dates plus the newest `weekly_count` Sundays,
delete the rest. Runs only after a successful new artifact lands;
a failed run never prunes. File mtime is not used (NAS clock skew
makes it unreliable).

**Rationale**:

- Filename-as-truth is robust across mount-points, copy operations,
  and timezone changes.
- "Prune only after success" preserves the invariant that there is
  always at least one complete snapshot on disk.
- Selecting Sundays for the weekly cohort is the conventional
  homelab pattern and is easy to reason about visually
  (`ls | grep -E '\b(2026-(05|06|07)-\w+)\b'`).

**Alternatives considered**:

- mtime-based pruning — fragile across NAS mounts; rejected.
- Symlinked "latest" + cohort directories — adds complexity for no
  operator-visible win; rejected.
- A separate retention metadata DB — would persist what the
  filename already says; rejected (Constitution VI).

## R-007 — Grafana panel set

**Decision**: Hand-author `deploy/grafana/klams.json` covering the
panels below. Klams ships the JSON file + a handoff document at
`~/ansible-k/specs/klams-integration/klams-grafana.md`. Operational
provisioning (datasource binding, dashboard upload via Grafana's
filesystem provisioning, and the two recommended alerts
`klams_backup_stale` / `klams_backup_failures`) is delegated to
ansible-k, which already owns the kubsdb Grafana instance per its
sprint `013-grafana-observability-stack`.

- Queue depth + worker utilization (existing series:
  `klams_event_queue_depth`, `klams_worker_active`).
- Write throughput by endpoint (existing
  `klams_http_requests_total{method,route,status}` — rate over 5m,
  filtered to non-GET).
- p50 / p95 / p99 latency for `/memory/search` and `/memory/context`
  (existing `klams_http_request_duration_seconds_bucket`).
- Error rate by status code (existing — same series, filtered to
  `status=~"5.."`).
- Last successful backup age — `time() - klams_backup_last_success_timestamp_seconds`
  rendered as a stat with thresholds at 24h (green) / 26h (amber) /
  48h (red).
- `klams_maintenance_mode_active` — stat with green/red color.
- `klams_summarization_lag_seconds` — sprint-005 series, included
  for completeness.

**Rationale**:

- All series either already exist or are added by this sprint
  (FR-006). No additional instrumentation work.
- Threshold of 26h on the last-success stat allows a backup to slip
  by up to 2 hours before the panel goes amber — wider than the
  ±2 minute SC-001 tolerance, so an SC-001 violation will show
  immediately and a clock-skew-only blip won't.
- Delegating provisioning to ansible-k keeps klams out of the
  Grafana-lifecycle business and gives operational changes
  (datasource rename, notifier swap, alert tuning) a single
  home in the infra repo where they belong.

**Panels** (all in `klams.json`):

**Alternatives considered**:

- Provision via Grafana's HTTP API at klams startup — couples klams
  to a Grafana instance, complicates dev environments; rejected.
- Use the Grafana "import dashboard from JSON" UI as the operator
  install path — fine for sprint-internal smoke testing, but
  fragile as the production install (manual, drifts across Grafana
  re-bootstraps); rejected for production in favor of the ansible-k
  handoff.
- Klams owns an ansible role itself — rejected; ansible-k is the
  single place for kubsdb infra config.

## R-008 — Restore validation approach

**Decision**: Implement `tests/integration/restore_roundtrip.rs`:
seed a known fixture into the test compose stack, run
`backup::run_once()`, tear down the stack, bring up a fresh stack,
run `restore::run_from(date)`, compare fact / event / knowledge
counts and a canonical 10-row sample. Cover SC-002. The same
integration test exercises FR-016 ("Restore MUST be exercised once
during this sprint").

**Rationale**:

- An integration test that mirrors the production-restore workflow
  IS the once-exercised restore, satisfying FR-016 without an
  out-of-band manual run.
- Failures here are loud (cargo-test red) and are noticed every
  CI run, not only when an operator remembers to test by hand.

**Alternatives considered**:

- Manual restore documented in a runbook only — fragile, drifts;
  rejected (would still need FR-016 evidence captured somewhere
  durable).
- Production-data restore drill — out of scope for a sprint test
  harness; the documented operator runbook covers this on demand.

## R-009 — Day-0 sizing micro-task

**Decision**: Add `tests/fixtures/scale_loader.rs` (or extend the
sprint-005 seed script) that loads ~10k facts, ~50k events, ~20k
knowledge chunks into the test compose stack, then `just backup-size`
runs `backup::run_once()` once with timing on. Output (a small table:
artifact, bytes, seconds) is printed to stdout and copied into a
note under `specs/006-maintenance-and-backups/sizing.md` (created
during sprint execution, referenced from quickstart.md). The
fixture is **also** the >=1k-fact corpus that the deferred
sprint-005 benchmarks T055/T056 (SC-001/SC-003/SC-004) need —
so this task discharges both debts.

**Rationale**:

- Without measured numbers, the retention defaults (14 daily +
  4 weekly) are guesses. The sizing run validates that the default
  fits "homelab disk" before code lands.
- Sharing the fixture with sprint-005's deferred benchmarks is pure
  upside — no extra fixture maintenance.

**Alternatives considered**:

- Skip sizing — risks operator surprise; rejected (Constitution
  V, Quality & Observability).
- Synthetic, statistically-shaped data (zipf distributions, etc.) —
  marginal benefit over uniform-ish fixtures for a one-off
  sizing exercise; rejected (Constitution VI).
