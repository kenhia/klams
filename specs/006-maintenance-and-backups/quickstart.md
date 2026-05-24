# Quickstart — Maintenance, Backups, and Ops (sprint 006)

This walkthrough lives alongside the spec/plan/research/data-model
and exercises the sprint deliverables end-to-end on a developer
machine using the existing `tests/docker-compose.test.yml` stack.
The final restore step (Section 5) is the once-exercised restore
that satisfies `FR-016`.

## Prerequisites

- Working `just gate` on `main` of branch `006-maintenance-and-backups`.
- The test compose stack up: `docker compose -f tests/docker-compose.test.yml up -d` (Postgres + Qdrant + TEI as usual).
- `pg_dump` and `pg_restore` (Postgres 16 client) on `$PATH`.
- A writable scratch directory: `mkdir -p /tmp/klams-backup-quickstart && export BACKUP_DIR=/tmp/klams-backup-quickstart`.

## 1. Configure `[backup]` in `klams.toml`

Edit your local `klams.toml` (or a copy used by the dev service):

```toml
[backup]
enabled = true
backup_dir = "/tmp/klams-backup-quickstart"
window_start_utc = "00:01"          # 1 minute past midnight UTC; near-immediate trigger
daily_count = 3
weekly_count = 1
same_day_strategy = "suffix"
status_hook = "/tmp/klams-hook.sh"
status_hook_timeout = "10s"
```

Drop a noop hook so we can watch the lifecycle:

```bash
cat > /tmp/klams-hook.sh <<'SH'
#!/usr/bin/env bash
ts=$(date -u +%FT%TZ)
echo "[$ts] event=$KLAMS_BACKUP_EVENT run_id=$KLAMS_BACKUP_RUN_ID"
cat
echo
SH
chmod +x /tmp/klams-hook.sh
```

Validate the config without starting the service:

```bash
just backup-validate-config
# expected: "OK: [backup] enabled=true backup_dir=/tmp/klams-backup-quickstart ..."
```

## 2. Trigger one backup on demand

The scheduler waits for `window_start_utc`. To exercise the path
right now without rolling the system clock, use the manual recipe
(which bypasses the scheduler but takes every other code path —
`MaintenanceState`, hook invocations, retention, metrics):

```bash
just backup-once
```

What should happen in order:

1. Log line: `INFO klams_service::backup::lifecycle: BackupRun started run_id=<ULID>`.
2. Hook stdout captured in journal: `[<ts>] event=started run_id=<ULID>` plus a JSON document with `"event": "started"`.
3. `pg_dump` log line with duration; `<BACKUP_DIR>/postgres-<UTC-date>.dump` lands atomically.
4. Qdrant snapshot log line with duration; `<BACKUP_DIR>/qdrant-<UTC-date>.snapshot` lands atomically.
5. Hook invoked again with `event=finished` and a JSON document containing both artifacts and `"ok": true`.

Verify:

```bash
ls -la "$BACKUP_DIR"
# expect: postgres-YYYY-MM-DD.dump and qdrant-YYYY-MM-DD.snapshot, both non-zero, no .partial files

curl -s localhost:8080/metrics | grep -E 'klams_backup_(last_success_timestamp|runs_total|duration_seconds_count|hook_invocations_total)'
# expect: klams_backup_runs_total{ok="true"} 1
#         klams_backup_hook_invocations_total{event="started",ok="true"} 1
#         klams_backup_hook_invocations_total{event="finished",ok="true"} 1
#         klams_backup_last_success_timestamp_seconds <epoch>
```

## 3. Observe maintenance-mode behavior

In one terminal, run an artificially-slow backup (override the
manual recipe to insert a 20s sleep between `pg_dump` and qdrant
snapshot — `just backup-once -- --inject-sleep 20`):

```bash
just backup-once -- --inject-sleep 20
```

In another terminal, during the window:

```bash
# Non-critical write: expect 503 + Retry-After + JSON envelope
curl -i -X POST localhost:8080/memory/facts \
  -H 'Content-Type: application/json' \
  -d '{"source":"Task","fact":"...","..."}'
# HTTP/1.1 503 Service Unavailable
# Retry-After: 30
# {"error":"maintenance_window_active","retry_after_seconds":30}

# Read: expect 200
curl -i localhost:8080/memory/facts?limit=1

# Critical write (User-source dissent resolution): expect 200
curl -i -X POST localhost:8080/memory/dissents/<id>/promote \
  -H 'X-Klams-Source: User'
```

After the backup completes, `/healthz` shows `maintenance.active: false` and writes succeed normally.

## 4. Misbehaving hook does not affect the backup

Replace the hook with a hanging one:

```bash
cat > /tmp/klams-hook.sh <<'SH'
#!/usr/bin/env bash
sleep 600
SH
```

Trigger another backup:

```bash
just backup-once
```

Expected:

- Both `started` and `finished` hook invocations time out after 10s.
- Backup artifacts still land in `$BACKUP_DIR` with the correct names.
- `klams_backup_hook_invocations_total{event="started",ok="false"}`
  and `{event="finished",ok="false"}` each increment by 1.
- `klams_backup_runs_total{ok="true"}` increments by 1 (the run
  itself succeeded; hook failure does not change the run's
  outcome).

## 5. Restore from a snapshot (FR-016)

This is the once-exercised restore — a SC-002 acceptance flow.

```bash
# 1. Record current state for comparison
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM facts;" -t > /tmp/pre-counts-facts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM events;" -t > /tmp/pre-counts-events

# 2. Tear down the live stack (lose all in-memory state)
docker compose -f tests/docker-compose.test.yml down -v

# 3. Bring up a fresh stack
docker compose -f tests/docker-compose.test.yml up -d

# 4. Wait for Postgres + Qdrant to be ready
just wait-for-stack    # (existing recipe; busy-loops on health checks)

# 5. Restore from yesterday's snapshot
just restore-from $(date -u -d 'yesterday' +%F)
# expected stdout:
#   restoring postgres from /tmp/klams-backup-quickstart/postgres-2026-05-22.dump...
#   restoring qdrant from /tmp/klams-backup-quickstart/qdrant-2026-05-22.snapshot...
#   restore complete

# 6. Compare counts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM facts;" -t > /tmp/post-counts-facts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM events;" -t > /tmp/post-counts-events
diff /tmp/pre-counts-facts /tmp/post-counts-facts && echo "facts match"
diff /tmp/pre-counts-events /tmp/post-counts-events && echo "events match"
```

`just restore-from` refuses to run if the target Postgres has any
rows in `facts` / `events` / `knowledge_items`, unless `--force`
is passed:

```bash
just restore-from 2026-05-22                  # fails: "target is non-empty; pass --force to overwrite"
just restore-from 2026-05-22 --force          # succeeds
```

## 6. Inspect the Grafana dashboard (sprint-internal smoke test)

This sprint validates the JSON file by importing it manually into a
running Grafana. **Production install lives in ansible-k**, not
here \u2014 the handoff document at
[`~/ansible-k/specs/klams-integration/klams-grafana.md`](../../../../ansible-k/specs/klams-integration/klams-grafana.md)
defines the contract ansible-k consumes.

```bash
# Load the dashboard JSON into any reachable Grafana (test stack or
# the live kubsdb instance once ansible-k has it provisioned).
curl -X POST -H "Content-Type: application/json" \
  -d @deploy/grafana/klams.json \
  http://admin:admin@localhost:3000/api/dashboards/db
```

Verify panels:

- "Last backup age" shows a value in seconds (< 60s right after step 2/4).
- "Maintenance mode" is green (off).
- "Backup runs" stacked counter shows `ok="true"` >= the number of successful runs from steps 2-5.

Then confirm the handoff document is in place and lists every series the panels reference:

```bash
test -f ~/ansible-k/specs/klams-integration/klams-grafana.md && \
  grep -c 'klams_backup_' ~/ansible-k/specs/klams-integration/klams-grafana.md
# expect a non-zero count covering at least the 5 backup-related series
```

## Acceptance checklist (Phase 5 exit)

- [X] SC-001 — Scheduled backup produces both artifacts within ±2 min of `window_start_utc`. (Validated by an integration test that fast-forwards `tokio` time.)
- [X] SC-002 — Restore reproduces production counts. (Section 5 + FR-016 evidence below; `restore_roundtrip.rs` integration test.)
- [X] SC-003 — Non-critical 503 / read 200 / critical 200 during the window. (Section 3; `maintenance_middleware.rs` integration tests.)
- [X] SC-004 — kpidash widget reflects start within 2s. (Out-of-tree on the kpidash side; klams meets its half of the contract via the `< 500ms` `started`-hook spawn budget asserted in `backup_status_hook.rs`.)
- [X] SC-005 — Misbehaving hook does not block the run. (Section 4; `backup_status_hook.rs` covers missing-exec, exit-1, and SIGTERM-bounded timeout.)
- [X] SC-006 — Grafana dashboard imports clean, panels render. (Section 6; manual import checklist in `dashboard-smoke.md`; `grafana_dashboard_json.rs` enforces panel/series invariants.)
- [X] SC-007 — `docs/setup.md` "Restore from snapshot" section walks an operator through Section 5 verbatim. (`docs/setup.md` "Sprint 006 — Restore from snapshot".)
- [X] SC-008 — ansible-k handoff at `~/ansible-k/specs/klams-integration/klams-grafana.md` exists, lists every series the panels consume, and is referenced from `plan.md` + `docs/architecture.md`. (Cross-reference lives in `docs/architecture.md` §2d; series-coverage enforced by `grafana_dashboard_json.rs`.)
- [X] `just gate` green.
- [X] Sprint-005 deferred T055/T056 benchmarks now have a >=1k-fact fixture (from R-009) and can be re-evaluated. (Pointer added to `specs/005-advanced-retrieval/tasks.md`; bench harness deferred to a follow-up sprint.)

## FR-016 evidence — once-exercised restore drill

**Status: PASSED on 2026-05-23.**

The end-to-end restore drill that satisfies FR-016 (and the SC-002
acceptance bullet) is automated as the integration test
[`crates/klams-service/tests/restore_roundtrip.rs`](../../crates/klams-service/tests/restore_roundtrip.rs)
(task T029, landed in commit `935a899` —
*"feat(backup): phase 4 US2 — restore from snapshot pair (T029-T034)"*,
2026-05-23). The test:

1. Brings up the `tests/docker-compose.test.yml` stack
   (Postgres 16 on :55432, Qdrant 1.12.4 on :56333/:56334).
2. Seeds the scale fixture from T014 (~10k facts, ~50k events,
   ~20k knowledge chunks).
3. Calls `klams_service::backup::run_once`, then tears the stack
   down with `-v` and brings it back up empty.
4. Calls `klams_service::backup::restore::run_from(date, force=true)`
   against the fresh stack and asserts row counts on
   `facts` / `events` / `knowledge_items` plus a 10-row sample
   identity check.

It first ran clean on the same date as the implementing commit
(`935a899`, 2026-05-23). The Section 5 manual walkthrough above is
the operator-facing version of the same drill; running
`just restore-from <date>` against a backup pair invokes the same
`restore::run_from` code path the integration test covers. This
satisfies the spec.md "once-exercised restore" requirement (FR-016)
without committing to a recurring DR drill cadence.
