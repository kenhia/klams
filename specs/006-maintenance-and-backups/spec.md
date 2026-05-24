# Feature Specification: Maintenance, Backups, and Ops

**Feature Branch**: `006-maintenance-and-backups`  
**Created**: 2026-05-23  
**Status**: Draft  
**Input**: User description: "Phase 5 of specs/planning/plan.md — nightly pg_dump of klams to gratch NAS, qdrant snapshot to gratch during a maintenance window, maintenance_mode flag that rejects non-critical writes during the window, generic backup status hook (configurable executable receiving a JSON report on stdin) so kpidash and other widgets can subscribe without klams ever speaking their protocols, Grafana dashboards covering queue/throughput/latency/backup state, and a documented + once-exercised restore procedure. Cloud sync stays out of scope; backups land on gratch and gratch's existing chain handles offsite. Conversation tweaks: status_hook also fires at backup *start* (not just finish) so kpidash can show in-flight runs; window_start is configured in UTC (`window_start_utc`) to dodge DST."

This sprint operationalizes Phase 5 of [the master plan](../planning/plan.md):
"Maintenance, backups, and ops." After sprint 005, klams is feature-complete
through Phase 4 — facts, events, knowledge, dissents, hybrid retrieval,
summarization, and `POST /memory/context` are all live — but the service has
no scheduled backup, no documented restore path, no way for an operator to
quiesce the write side for a clean snapshot, and no way for the kpidash
operator dashboard to surface backup state without klams growing
dashboard-specific code. Phase 5 closes those gaps so klams is
production-stable in the homelab: backups run unattended every night, a
restore from yesterday's snapshot is a documented and tested procedure,
operators can observe queue/throughput/latency/backup state on a Grafana
dashboard, and a single configurable hook lets any external observer
(starting with kpidash) subscribe to backup lifecycle events without klams
linking against Redis, MQTT, or any other transport.

Cloud sync is **out of scope** and stays that way: backup artifacts land on
the gratch NAS and the existing gratch backup chain handles the offsite
copy. Klams ships zero cloud-provider plumbing — that boundary is the file
sitting in the configured backup directory.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Unattended nightly backup of klams state (Priority: P1)

Ken (the operator) wants to walk away from the homelab and trust that
klams's facts, events, knowledge, and provenance metadata are being
snapshotted nightly to the gratch NAS — without him remembering to run
anything, without the snapshot being silently incomplete, and without
the backup competing with daytime read/write traffic. Today, klams
has no scheduled backup at all: a host failure would lose every
fact and every embedded knowledge chunk since the last manual
`pg_dump`. Phase 5 ships a backup job (Postgres `pg_dump` + Qdrant
snapshot) that runs once per configured maintenance window, writes
atomically into the configured backup directory, retains a rolling
window of daily + weekly snapshots, and exposes Prometheus metrics
so a missed or failed run shows up on the operator dashboard.

**Why this priority**: This is the single deliverable the Phase 5
exit criterion calls out by name ("backup runs nightly without
intervention"). Every other story in this sprint either feeds this
one (maintenance mode for consistency) or observes it (status hook,
Grafana dashboards, restore). P1.

**Independent Test**: Configure `[backup]` with `window_start_utc =
"10:00"` and a writable `backup_dir`. Within 24 hours, the directory
contains a fresh `postgres-YYYY-MM-DD.dump` and `qdrant-YYYY-MM-DD.tar.zst`
(or equivalent qdrant-native snapshot format), each within ±2 minutes
of the configured window start, both files atomically present (no
`.partial` left over), the `klams_backup_last_success_timestamp_seconds`
metric matches the file's mtime, and no operator intervention occurred.

**Acceptance Scenarios**:

1. **Given** a configured backup window and a writable backup directory,
   **When** the window opens, **Then** klams produces both a Postgres
   dump and a Qdrant snapshot, both files appear atomically (rename
   from a `.partial` suffix only after success), and both file sizes
   are non-zero.
2. **Given** a backup directory containing older snapshots, **When**
   a new backup completes, **Then** the retention policy (default
   14 daily + 4 weekly) prunes excess snapshots only after the new
   file lands; no live snapshot is ever the only copy on disk
   during the prune.
3. **Given** Postgres is reachable but Qdrant is down, **When** the
   window opens, **Then** the Postgres dump still completes, the
   Qdrant artifact is recorded as failed in the status hook payload,
   the run as a whole is marked `ok: false`, and the next window
   retries without backoff.
4. **Given** a backup that ran successfully, **When** an operator
   queries `/metrics`, **Then** `klams_backup_last_success_timestamp_seconds`
   exposes the unix epoch of the most recent successful run and
   `klams_backup_duration_seconds{kind="postgres"|"qdrant"}` exposes
   the wall-clock duration of each artifact's portion.

---

### User Story 2 — Restore klams from yesterday's snapshot (Priority: P1)

Ken wants confidence that the nightly backup is restorable — not in
theory, but actually exercised. Today there is no restore procedure;
a host failure would force ad-hoc recovery from a `pg_dump` file
nobody has practiced reading. Phase 5 ships a documented restore
runbook (Postgres + Qdrant) and exercises it at least once: on a
clean staging stack, applying yesterday's snapshot reproduces the
fact / event / knowledge counts, and a small set of canonical
`/memory/search` and `/memory/context` queries return the same
bundles pre- and post-restore.

**Why this priority**: A backup you have never restored is not a
backup. P1.

**Independent Test**: On a clean compose stack (empty Postgres,
empty Qdrant), copy yesterday's `postgres-YYYY-MM-DD.dump` and
`qdrant-YYYY-MM-DD.tar.zst` into place, run `just restore-from
YYYY-MM-DD`, and observe that `SELECT COUNT(*) FROM facts; … FROM events; …`
and the qdrant collection count match the production counts at the
snapshot timestamp ± any in-flight writes from the maintenance
window. A `GET /memory/facts?limit=10` returns the same first 10
facts as on production at snapshot time.

**Acceptance Scenarios**:

1. **Given** a snapshot pair from a known production timestamp,
   **When** an operator runs `just restore-from <date>` against a
   clean stack, **Then** facts / events / knowledge counts match the
   production counts at that timestamp.
2. **Given** a partially-corrupt snapshot (e.g. truncated dump),
   **When** restore runs, **Then** `pg_restore` reports an error,
   `just restore-from` exits non-zero, and no partial state is left
   in the target Postgres database (use `pg_restore --single-transaction`
   or equivalent guarantee).
3. **Given** the restore procedure is documented in `docs/setup.md`,
   **When** an operator follows the steps verbatim, **Then** the
   procedure completes without requiring tribal knowledge or
   out-of-band steps.

---

### User Story 3 — Backup window quiesces non-critical writes (Priority: P2)

While `pg_dump` is running and the Qdrant snapshot is being created,
inflight write traffic could leave the two snapshots split across an
application-level invariant spanning Postgres + Qdrant. Phase 5
ships a `maintenance_mode` flag that flips ON for the duration of
the backup window: non-critical writes (fact upserts, event appends,
knowledge indexing) return `503 Service Unavailable` with a
`Retry-After` header; critical writes (dissent promote/discard
initiated by a `User`-source request) and all read endpoints
continue to serve. The flag is owned by klams (flipped by the
backup task), not by an operator toggle; it flips OFF when the
backup completes — even if it overruns its window.

**Why this priority**: This upgrades Story 1 from "a backup" to "a
consistent backup." P2 because Story 1 still delivers value on its
own.

**Independent Test**: With backup configured to run, send a
`POST /memory/facts` request at the moment the window opens. The
response is `503` with `Retry-After: <integer seconds>` and a JSON
error envelope naming `"reason": "maintenance_window_active"`. Send
`GET /memory/facts` during the same window — response is `200`.
Send `POST /memory/dissents/{id}/promote` from a `User`-source
request — response is `200`. After backup completes, the same
`POST /memory/facts` returns `200`.

**Acceptance Scenarios**:

1. **Given** `maintenance_mode` is active, **When** an agent posts
   a non-critical write, **Then** the response is `503` with
   `Retry-After` and `reason=maintenance_window_active`.
2. **Given** `maintenance_mode` is active, **When** any client
   reads (`GET /memory/facts`, `POST /memory/search`,
   `POST /memory/context`), **Then** the response is normal.
3. **Given** `maintenance_mode` is active, **When** a `User`-source
   request promotes or discards a dissent, **Then** the write
   proceeds.
4. **Given** the backup completes, **When** any subsequent write
   arrives, **Then** `maintenance_mode` is OFF and the write
   succeeds.
5. **Given** `klams_maintenance_mode_active` is scraped, **When**
   the window is OFF / ON, **Then** the gauge reads `0` / `1`.

---

### User Story 4 — kpidash subscribes to backup lifecycle via a generic hook (Priority: P2)

Ken runs an operator dashboard (`~/src/tools/kpidash`) that
publishes via Redis. He wants the dashboard to show "backup
running" and "backup finished at HH:MM" widgets without klams
linking against Redis, MQTT, or any other dashboard transport. The
Phase 5 design: klams takes one optional config value,
`[backup] status_hook = "/path/to/executable"`. When configured,
klams invokes the executable at three lifecycle points
(`started`, `finished`, `failed`) and pipes a stable, versioned
JSON report on stdin. The hook's exit code, stdout, and stderr
are captured in the klams journal but are **never** enforced — a
broken widget cannot fail the backup. Ken writes a tiny shim
(`klams-backup-status`) in the kpidash tree that reads the JSON,
publishes to Redis, and exits 0. Klams ships zero
kpidash-specific code; the contract is JSON-on-stdin + exit code.

The `started` event is part of the lifecycle (not just `finished` /
`failed`) so kpidash can show in-flight runs: pg_dump on a growing
dataset could legitimately take long enough that the operator wants
to see "started 7 minutes ago" rather than wait for completion.

**Why this priority**: This is the operator-visibility win. P2
because the backup runs correctly whether or not a hook is
configured, and the hook can be added / changed / removed without
redeploying klams.

**Independent Test**: Configure `status_hook = "/tmp/klams-hook.sh"`
pointing at a tiny shell script that appends stdin to a log file.
Trigger a manual backup. The log file contains two JSON objects in
order — `event: "started"` followed by either `event: "finished"`
(if all artifacts succeeded) or `event: "failed"` (if any failed).
Both objects share the same `run_id` and have monotonically
non-decreasing timestamps. Pointing the hook at a missing
executable or at a script that hangs longer than
`status_hook_timeout` does not affect the backup outcome.

**Acceptance Scenarios**:

1. **Given** `status_hook` is configured, **When** the backup window
   opens, **Then** the hook is invoked once with `event: "started"`
   *before* `pg_dump` begins.
2. **Given** all artifacts complete successfully, **When** the
   backup finishes, **Then** the hook is invoked once with
   `event: "finished"`, `ok: true`, and per-artifact entries with
   `bytes`, `duration_ms`, and `path`.
3. **Given** at least one artifact fails, **When** the backup
   finishes, **Then** the hook is invoked once with
   `event: "failed"`, `ok: false`, and an `error` field naming
   the first failure.
4. **Given** the hook executable is missing, hangs past
   `status_hook_timeout`, or exits non-zero, **When** the backup
   runs, **Then** the artifacts still land, the failure is logged,
   `klams_backup_hook_invocations_total{event,ok="false"}` increments,
   and the next lifecycle event is still attempted (a failed
   `started` hook does not skip the `finished` call).
5. **Given** `status_hook` is unset, **When** the backup runs,
   **Then** no hook is invoked, no warning is logged, and the
   backup completes normally.

---

### User Story 5 — Grafana shows queue, throughput, latency, and backup state (Priority: P3)

Ken wants one place to glance and confirm klams is healthy. Phase 5
ships a Grafana dashboard JSON under `deploy/grafana/klams.json`
and a handoff document so the `ansible-k` project (which owns the
kubsdb Grafana instance per its sprint
`013-grafana-observability-stack`) can provision the dashboard,
datasource binding, and any panel-derived alerts. Klams itself is
not in the dashboard-provisioning business — klams ships the JSON
file + a contract for the Prometheus series it exposes;
ansible-k's role pulls the file from a pinned klams release tag
and wires it into Grafana. The panel set covers queue depth +
worker utilization, write throughput by endpoint, p50 / p95 / p99
latency for `/memory/search` and `/memory/context`, error-rate by
status code, the `klams_backup_last_success_timestamp_seconds`
age, the `klams_maintenance_mode_active` gauge, and the
`klams_summarization_lag_seconds` gauge from sprint 005.

During this sprint we validate the JSON by manually importing it
into a Grafana (test stack or live kubsdb instance) so we catch
shape errors before handing off. Operational deployment +
alerting are tracked in [`~/ansible-k/specs/klams-integration/klams-grafana.md`](../../../../ansible-k/specs/klams-integration/klams-grafana.md).

**Why this priority**: A dashboard is operator polish — `/metrics`
exposes the raw series even without it. P3.

**Independent Test**: Import `deploy/grafana/klams.json` into a
running Grafana that is scraping the kubs0 Prometheus exporter
(during sprint development; the production install path is
ansible-k's role). Every panel renders without "No data" for the
relevant series; the "Last successful backup" stat panel reads
`< 26h` immediately after a successful nightly run; the
"Maintenance mode" panel goes red while a backup is running and
green otherwise.

**Acceptance Scenarios**:

1. **Given** a fresh Grafana instance scraping the klams Prometheus
   exporter, **When** the operator imports `deploy/grafana/klams.json`,
   **Then** every panel resolves its data source and renders the
   correct series.
2. **Given** a backup completed successfully within the last day,
   **When** the operator views the dashboard, **Then** the "Last
   backup age" stat reads `< 26h`.
3. **Given** klams is in `maintenance_mode`, **When** the operator
   views the dashboard, **Then** the "Maintenance mode" panel
   shows the active state.
4. **Given** the dashboard JSON is committed at
   `deploy/grafana/klams.json`, **When** the sprint ships, **Then**
   the ansible-k handoff document
   ([`~/ansible-k/specs/klams-integration/klams-grafana.md`](../../../../ansible-k/specs/klams-integration/klams-grafana.md))
   lists every series the panels reference, the two suggested
   alerts (`klams_backup_stale`, `klams_backup_failures`), and
   the acceptance criteria ansible-k will validate against.

---

### Edge Cases

- **Window overrun**: a backup that runs longer than its window
  duration. Klams stays in `maintenance_mode` until completion —
  the configured window is the *start trigger*, not a hard ceiling.
  Grafana surfaces overrun visually; no automated kill.
- **Service restart mid-backup**: the in-progress backup is
  abandoned (its `.partial` file remains for the next run to
  overwrite). If klams can detect a pending run via lockfile on
  startup, it fires the hook with `event: "failed"`,
  `error: "service_restarted_mid_backup"`.
- **Disk full on backup_dir**: `pg_dump` / qdrant snapshot fails;
  retention does *not* prune older snapshots in the same run (we
  never delete to make room); hook fires `failed`; counter
  increments; operator sees it on the dashboard.
- **status_hook stalls forever**: bounded by
  `status_hook_timeout` (default `10s`). Klams sends SIGTERM,
  waits 2s, then SIGKILL.
- **Restore over a populated stack**: `just restore-from` refuses
  unless `--force` is passed.
- **Two backup runs on the same day**: collision policy lives in
  `[backup] same_day_strategy = "suffix" | "overwrite"`,
  default `"suffix"` (file gets `-N` suffix).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: klams MUST run a backup job once per configured
  maintenance window, producing one Postgres dump and one Qdrant
  snapshot per run, both landing atomically in the configured
  `backup_dir` with date-stamped filenames.
- **FR-002**: `pg_dump` MUST use a custom format (`-Fc`) suitable
  for `pg_restore` and MUST be invoked with credentials sourced
  from the same config block already used by the service.
- **FR-003**: The Qdrant snapshot MUST be taken via Qdrant's
  native snapshot API and downloaded to `backup_dir` (not left in
  qdrant's internal storage).
- **FR-004**: Backup files MUST be written to
  `<backup_dir>/<name>.partial` first and renamed to the final
  name only after the source operation reports success. A
  `.partial` file from a previous failed run MUST be overwritten
  by the next attempt.
- **FR-005**: Retention MUST keep the most recent `daily_count`
  (default 14) date-stamped files of each kind, plus
  `weekly_count` (default 4) files dated to Sunday. Pruning MUST
  run only after a successful new artifact lands; a failed run
  never prunes.
- **FR-006**: klams MUST expose Prometheus metrics:
  - `klams_backup_last_success_timestamp_seconds` (gauge, unix epoch)
  - `klams_backup_duration_seconds{kind="postgres"|"qdrant"}` (histogram)
  - `klams_backup_runs_total{ok="true"|"false"}` (counter)
  - `klams_backup_hook_invocations_total{event,ok}` (counter)
  - `klams_maintenance_mode_active` (gauge 0/1)
- **FR-007**: While the backup is in flight, klams MUST set
  `maintenance_mode = true`. Non-critical writes (any
  state-mutating verb against fact / event / knowledge endpoints
  other than dissent `promote` / `discard` from a `User`-source
  request) MUST respond `503 Service Unavailable` with header
  `Retry-After` (integer seconds) and a JSON error envelope
  `{"error":"maintenance_window_active","retry_after_seconds":N}`.
- **FR-008**: Reads (any `GET`, `POST /memory/search`,
  `POST /memory/context`) MUST NOT be gated by
  `maintenance_mode`.
- **FR-009**: When `[backup] status_hook` is set, klams MUST
  invoke the executable at `started`, `finished`, and `failed`
  lifecycle points, passing a UTF-8 JSON document on stdin
  conforming to the contract under
  `contracts/backup-status-hook.schema.json`. Stdout and stderr
  from the hook MUST be captured in the journal (truncated to
  4 KiB each).
- **FR-010**: Hook invocations MUST be bounded by
  `[backup] status_hook_timeout` (default `10s`). On timeout,
  klams sends SIGTERM, waits 2s, then SIGKILL. A hook timeout or
  non-zero exit MUST NOT change the backup outcome — the
  artifacts still land, the next lifecycle event is still
  attempted, and the failure is recorded only in the journal and
  `klams_backup_hook_invocations_total{ok="false"}`.
- **FR-011**: The status-hook JSON document MUST include a
  top-level `schema_version` (integer; this sprint ships `1`) so
  future field additions can be made backwards-compatibly.
- **FR-012**: Configuration MUST live in `klams.toml` under a
  new `[backup]` block. `window_start_utc` is a string in
  `HH:MM` 24h UTC. There is no `window_start_local` — UTC only,
  to dodge DST. `enabled = false` (default) skips the backup
  task entirely; setting `enabled = true` without a writable
  `backup_dir` MUST fail validation at service start (exit code
  `2`, same convention as the sprint-005 decay-config validator).
- **FR-013**: A `just restore-from <YYYY-MM-DD>` recipe MUST
  exist that restores both Postgres and Qdrant from the dated
  artifact pair in `backup_dir`. The recipe MUST refuse to run
  against a non-empty target stack unless `--force` is passed.
- **FR-014**: The restore procedure MUST be documented in
  `docs/setup.md` with verbatim commands under a "Restore from
  snapshot" section.
- **FR-015**: A Grafana dashboard JSON file MUST live at
  `deploy/grafana/klams.json` and reference only series that
  klams's `/metrics` exposes after this sprint lands. It MUST
  cover at minimum: queue depth + worker utilization, write
  throughput by endpoint, p50/p95/p99 latency for
  `/memory/search` and `/memory/context`, error rate by status
  code, last-backup-success age, `maintenance_mode` state, and
  `summarization_lag_seconds`. The JSON MUST be importable into a
  vanilla Grafana ≥ 10 without manual edits (validated by a
  manual import during this sprint).
- **FR-016**: Restore MUST be exercised once during this sprint
  on a clean compose stack — the walkthrough lives in
  `specs/006-maintenance-and-backups/quickstart.md` as part of
  the acceptance gate.
- **FR-017**: A handoff document MUST exist at
  `~/ansible-k/specs/klams-integration/klams-grafana.md` (i.e. in
  the ansible-k repo, not the klams repo) describing the dashboard
  JSON, the Prometheus series it consumes, the two recommended
  alerts (`klams_backup_stale` on stale
  `klams_backup_last_success_timestamp_seconds`,
  `klams_backup_failures` on
  `klams_backup_runs_total{ok="false"}`), and the acceptance
  criteria ansible-k will validate against. Klams ships the JSON +
  the handoff; ansible-k owns provisioning, datasource binding,
  and notifier routing.
- **FR-018**: The `/healthz` response MUST include an additive
  `maintenance` block: `{ "active": bool, "run_id"?: str,
  "started_at"?: rfc3339, "expected_end_at"?: rfc3339 }`. When
  no backup is in flight the block is `{"active": false}`. All
  other `/healthz` fields are unchanged.

### Out of Scope

- **Cloud sync**: klams does not push to S3 / B2 / GCS / rsync.net /
  etc. The backup directory is a filesystem path; whatever copies
  it offsite is someone else's job (gratch handles it).
- **Point-in-time recovery**: `pg_dump` snapshots only — no WAL
  archiving, no logical replication. PITR is future work in
  `specs/planning/backlog.md`.
- **Encrypted backups**: artifacts are written in the clear; if
  encryption is wanted, layer it on the gratch side.
- **Hot Qdrant replication**: snapshot-and-copy only.
- **Hook event types beyond `started` / `finished` / `failed`**:
  a future sprint may add `compaction`, `restore`, etc.; this
  sprint reserves the namespace by versioning the schema.

### Key Entities

- **BackupRun**: one execution of the backup task. Fields:
  `run_id` (ULID), `started_at`, `ended_at`, `ok`, `artifacts[]`,
  `error` (nullable). In-memory only — not persisted to Postgres;
  the source of truth is `backup_dir` file mtimes plus the
  Prometheus series.
- **BackupArtifact**: one file produced by a `BackupRun`. Fields:
  `kind` (`"postgres"` | `"qdrant"`), `path`, `bytes`,
  `duration_ms`, `ok`, `error` (nullable).
- **MaintenanceState**: derived runtime state (named
  `MaintenanceState` in code to match the implementation struct).
  Fields: `active` (bool), `started_at`, `expected_end_at`
  (best-effort, based on last run's duration), `run_id`. Exposed
  via the `klams_maintenance_mode_active` metric and the
  `/healthz.maintenance` block (FR-018).

## Success Criteria *(mandatory)*

- **SC-001**: A backup window configured to run within the next
  hour produces both artifacts in `backup_dir` within ±2 minutes
  of the window start, without any operator action.
- **SC-002**: Restoring from yesterday's snapshot on a clean
  compose stack reproduces the production fact / event /
  knowledge_item counts (modulo any in-flight writes from the
  maintenance window itself).
- **SC-003**: During a backup, a `POST /memory/facts` from a
  `Task`-source returns `503 + Retry-After` while a concurrent
  `GET /memory/facts` returns `200`.
- **SC-004**: With `status_hook` configured to point at a shim
  that publishes to Redis, the kpidash widget shows "backup
  running" within 2 seconds of window open and "backup finished"
  within 2 seconds of completion.
- **SC-005**: A misconfigured hook (missing executable, infinite
  loop, exit 1) does not affect SC-001 — backup artifacts still
  land on time.
- **SC-006**: The Grafana dashboard renders all panels with live
  data immediately after import (sprint-internal manual import
  against a test or kubsdb Grafana); the "Last backup age" panel
  reads `< 26h` after a successful nightly run.
- **SC-007**: The restore walkthrough in `docs/setup.md` is
  followable by an operator who has not seen klams's internals,
  using only the documented `just` recipes.
- **SC-008**: The ansible-k handoff document
  ([`~/ansible-k/specs/klams-integration/klams-grafana.md`](../../../../ansible-k/specs/klams-integration/klams-grafana.md))
  exists, lists every series the dashboard consumes, and is
  cross-referenced from this sprint's `plan.md` and from
  `docs/architecture.md`. Operational deployment + alerting are
  out of scope for klams (ansible-k owns them) but the handoff
  contract is in scope.
