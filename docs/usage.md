# klams-service operator guide

This document covers the runtime behaviour of `klams-service`:
authentication, health/metrics endpoints, exit codes, and the
recommended systemd deployment. For HTTP request/response shapes
see [sprints/001-initial-mvp/contracts/openapi.yaml](../sprints/001-initial-mvp/contracts/openapi.yaml).

## Authentication

All `/memory/*` routes require a bearer token. The token is loaded
from configuration (`auth.bearer_token` in the service config or
`KLAMS_AUTH_BEARER_TOKEN` env var) and compared in constant time.

```text
Authorization: Bearer <token>
```

`/healthz` and `/metrics` are **unauthenticated** so probes and
scrapers don't need credentials.

## Health endpoint

`GET /healthz` returns a `HealthSnapshot` JSON document and a
status code matching the aggregate state:

| Aggregate | HTTP | Meaning |
|-----------|------|---------|
| `Ok`       | 200  | All subsystems healthy. |
| `Degraded` | 503  | At least one subsystem reports `Degraded`, none are `Down`. |
| `Down`     | 503  | At least one subsystem is `Down`. |

Subsystems probed: Postgres (`SELECT 1`), Qdrant (collection list),
embedder (`GET {tei_url}/health`). Each probe result is cached for
~2s to absorb scrape storms (Kubernetes liveness/readiness, dashboards).

Sample (healthy) response:

```json
{
  "status": "Ok",
  "postgres":   { "state": "Ok" },
  "qdrant":     { "state": "Ok" },
  "embeddings": { "state": "Ok" },
  "queue":      { "depth": 0, "capacity": 256, "workers": 2 },
  "version":    "0.1.0",
  "uptime_seconds": 1234
}
```

The Rust client surfaces this via [`klams_client::Client::health`](../crates/klams-client/src/lib.rs)
which deserializes the snapshot regardless of whether the HTTP
status was 200 or 503, letting callers display degraded subsystems
instead of just a transport error.

## Metrics endpoint

`GET /metrics` exposes the Prometheus text exposition format,
including the standard axum-prometheus HTTP histograms and the
named klams metrics below.

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `klams_queue_depth` | gauge | – | Current queued write jobs. |
| `klams_queue_capacity` | gauge | – | Configured queue capacity. |
| `klams_workers_active` | gauge | – | Active worker tasks. |
| `klams_writes_accepted_total` | counter | `type=fact\|event\|knowledge` | Writes accepted onto the queue. |
| `klams_writes_failed_total` | counter | `type`, `reason=queue_full\|store_error\|too_large` | Writes rejected or failed downstream. |
| `klams_write_latency_seconds` | histogram | `type` | Handler-side latency from validation to enqueue completion. |
| `klams_retrieval_duration_seconds` | summary (quantile label) | `op=search\|context`, `transport=rest\|mcp` | Retrieval latency at every entry point — REST and MCP (sprint 020, WI 63; replaces `klams_search_latency_seconds` / `klams_context_request_seconds`). |
| `klams_embedding_latency_seconds` | histogram | – | Latency of a single TEI call. |

## Exit codes

| Code | Cause |
|------|-------|
| `0`  | Clean shutdown (SIGTERM/SIGINT after `tokio::signal`). |
| `1`  | Configuration error (missing/invalid `klams-service.toml` or env). |
| `2`  | Dependency error at boot (Postgres unreachable, Qdrant connect failed, embedder schema mismatch). |
| `64` | Invalid CLI arguments (per `sysexits.h` convention). |

## systemd

Suggested unit (`/etc/systemd/system/klams.service`):

```ini
[Unit]
Description=klams memory service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=klams
Environment=KLAMS_CONFIG=/etc/klams/service.toml
ExecStart=/usr/local/bin/klams-service
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Operate with:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now klams.service
sudo systemctl status klams.service
journalctl -u klams.service -f      # tail logs
journalctl -u klams.service --since "1 hour ago"
```

## Dissent lifecycle (sprint 002)

A **dissent** is a lower-trust write (typically `AgentProposal`) that
contradicts an existing higher-trust canonical fact (`User`,
`Controller`). The service diverts these into a separate `dissents`
table instead of overwriting, so an operator can review them.

```text
   POST /memory/facts                              POST /memory/dissents/{id}/promote
   (AgentProposal vs User-owned (type, key))            │
        │                                               ▼
        ▼                                          canonical Fact replaced
   202 { dissent_id, status: "pending" }           (version++, source = promoter)
        │                                          dissent.status = "promoted"
        │
        ├──▶ GET /memory/dissents?status=pending   ─┐
        │       (filter by fact_id, source, …)      │
        │                                           │
        └──▶ POST /memory/dissents/{id}/discard  ───┘
                dissent.status = "discarded"
                fact unchanged
```

Default fact reads (`GET /memory/facts/...`) expose a
`dissent_count` so pending proposals are discoverable from any list
or detail view.

| Endpoint | Auth role | Effect |
|----------|-----------|--------|
| `GET  /memory/dissents` | any bearer | Paginated list; filterable by `status`, `source`, `fact_id`, `created_after`, `caller_source`. |
| `GET  /memory/dissents/{id}` | any bearer | Single dissent (proposed payload, source, timestamps, dedupe count). |
| `POST /memory/dissents/{id}/promote` | `User` or `Controller` | Replaces canonical fact, bumps `version`, sets `source` to promoter. Requires `expected_version`. 409 on stale version, 410 if dissent was already resolved, 403 if request `source` is below promote threshold. |
| `POST /memory/dissents/{id}/discard` | `User` or `Controller` | Marks dissent `discarded`. Fact untouched. Same 403/410 rules. |

The Phase 2 quickstart walks the full flow:
[sprints/002-safety-and-write-ops/quickstart.md §5](../sprints/002-safety-and-write-ops/quickstart.md#5-story-2--dissent-on-lower-trust-contradiction-promote-later).

## Viewport: provenance panel + Dissents page

The Phase 2 viewport surfaces two new affordances on top of the
existing read-only inspectors:

- **Provenance panel** — on every fact / event / knowledge detail
  view. Renders the eight provenance fields (`source`, `version`,
  `created_at`, `updated_at`, `last_used_at`, `decay_weight`,
  `confidence`, `dissent_count`) as a definition list. When
  `dissent_count > 0` it links to `/dissents?fact_id=…` so the
  operator can review the pending proposals.
- **Dissents page** (`/dissents`, in the top nav) — paginated review
  queue with filters (status, source, fact_id, created_after,
  caller_source) and per-row diff between the canonical fact and the
  proposed payload. Each row exposes **Promote** and **Discard**
  buttons that call the endpoints above with optimistic UI rollback
  on backend error.

The Facts inspector also gains **Edit** and **Delete** actions
(User-sourced) with optimistic application and rollback when the
backend reports `409 version_conflict` or any error envelope. A
mutation counter in the layout drives a pending-dissent badge in the
nav bar so newly diverted writes show up without a manual refresh.

## `just` recipe reference

Sprint 002 introduces a top-level [`justfile`](../justfile); every
common task is a one-liner that matches what CI runs.

| Recipe            | What it does |
|-------------------|--------------|
| `default`         | Prints the menu (`just --list`). |
| `compose-up`      | Bring `deploy/docker-compose.yml` up in the background. |
| `compose-down`    | Tear the stack down (keeps volumes). |
| `compose-rebuild` | `down` → `build --no-cache` → `up -d`. |
| `build`           | `cargo build -p klams-service --release`. |
| `run`             | `cargo run -p klams-service`, logs to stderr. |
| `test`            | `cargo test --workspace`. |
| `gate`            | Constitution pre-commit gate; identical to CI. |
| `health`          | `/healthz` curl + `scripts/verify-mvp.sh --light`. |
| `verify`          | Full `scripts/verify-mvp.sh` (SC-001..SC-009). |
| `viewport-build`  | `cargo xwin` Windows cross-build of the viewport. |
| `viewport-build-linux` | Native Linux build of the viewport (also runs in WSL Ubuntu). |
| `viewport-run-linux`   | Build + launch the Linux viewport with `--debug`. |

`KLAMS_URL` and `KLAMS_TOKEN` are read from the environment (with
local-dev defaults) so the same `just health` and `just verify`
work against a local stack or a remote `kubs0`.

> **WSL note**: The Linux viewport runs unchanged under WSL Ubuntu via
> WSLg. Install the webkit2gtk runtime first:
> `sudo apt install libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0 libsoup-3.0-0`.
> Quick-tested launching both `./klams-viewport` and `./klams-viewport --debug`.

## Reference

- HTTP contract: [openapi.yaml](../sprints/001-initial-mvp/contracts/openapi.yaml)
- Spec: [spec.md](../sprints/001-initial-mvp/spec.md) (US5 — Observability & Operations)
- Functional requirements FR-017..FR-020 cover `/healthz`, `/metrics`,
  exit codes, and journal output.

## Sprint 003 — write-path additions

### `path` field on write responses

Every write endpoint that previously returned a `Fact` or an event
shape now also returns a `path` field whose value is `"canonical"`
when the write landed on the canonical row and `"dissent"` when it
was diverted to the dissents queue (only possible for
`source: "AgentProposal"` writes contradicting a higher-trust row).

The field is **additive**. Pre-sprint-003 clients that ignored
unknown fields keep working — the `Fact` shape is flattened so
`response.id`, `response.version`, etc. still appear at the top
level.

```jsonc
// POST /memory/facts response (200)
{
  "id": "019e3e02-ff6e-75d1-b6d4-c821277539dc",
  "type": "UserFact",
  "version": 1,
  "payload": {"name": "Ken", "host": "kubs0"},
  "source": "User",
  // ... rest of Fact ...
  "path": "canonical"
}
```

### `GET /memory/policy`

Returns the runtime `MemoryPolicy` (dedupe rules, decay λ per
`FactType`, dissent thresholds) so callers can introspect server
behaviour without parsing the TOML:

```sh
curl -sS -H "Authorization: Bearer $KLAMS_TOKEN" \
    "$KLAMS_URL/memory/policy" | jq .
```

### Scanner one-shot

To trigger an ad-hoc scan of the configured roots without waiting
for the timer:

```sh
just scanner-once
# or directly:
sudo systemctl start klams-scanner.service
```

The scanner is idempotent: rerunning it on an unchanged tree posts
zero chunks.

### ansible-k handoff

The sprint-003 handoff document for the ansible-k integrator lives
at [sprints/003-non-agentic-writes/handoff/](../sprints/003-non-agentic-writes/handoff/).
It is self-contained markdown + a runnable POSIX `sh` example, ready
to `cp -r` to `/home/ken/ansible-k/specs/klams-integration/`.

## Sprint 003 — `just` recipe additions

| Recipe              | What it does |
|---------------------|--------------|
| `install-systemd`   | Build the three release binaries and run `deploy/install-systemd.sh` (idempotent; rotates `<bin>.prev`). |
| `scanner-once`      | `cargo run --release --bin klams-scanner -- --once`. |
| `monitor-once`      | `cargo run --release --bin klams-monitor -- --once`. |
| `rollback`          | Swap every `/usr/local/bin/<bin>` with its `.prev` and restart the long-running units. |

## Sprint 005 — `/memory/context` and decay tuning

### `POST /memory/context`

Returns a single token-budgeted bundle of facts + knowledge + events
that an agent can drop straight into a prompt. The wire shape is
defined in
[sprints/005-advanced-retrieval/contracts/memory-context.openapi.yaml](../sprints/005-advanced-retrieval/contracts/memory-context.openapi.yaml).

```sh
curl -fsS https://kubs0:7777/memory/context \
  -H "authorization: bearer $KLAMS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{
        "query": "how is GPU driver state tracked on kai?",
        "token_budget": 2048,
        "filters": { "host": "kai", "type": "EnvFact" }
      }' | jq .
```

Response sketch:

```json
{
  "facts":     [{ "kind": "raw",     "id": "...", "score": 0.91, "tokens": 142, "payload": {...} }],
  "knowledge": [{ "kind": "digest",  "id": "...", "score": 0.74, "tokens": 220, "payload": {...} }],
  "events":    [{ "kind": "summary", "id": "...", "score": 0.66, "tokens":  88, "payload": {...} }],
  "total_spent": 1880,
  "truncated": false,
  "token_encoder": "cl100k_base",
  "sections": {
    "facts":     { "count": 4, "tokens_spent":  568, "source": "raw",     "status": "ok" },
    "knowledge": { "count": 3, "tokens_spent": 1224, "source": "mixed",   "status": "ok" },
    "events":    { "count": 1, "tokens_spent":   88, "source": "summary", "status": "degraded",
                   "degraded_reason": "chat endpoint unreachable; fell back to extractive" }
  }
}
```

**Filter keys** (all optional, all AND-ed): `host`, `type`, `tag`,
`repo`, `file`, `source`, `since` (RFC 3339), `until` (RFC 3339).
Unknown filter keys return `400` naming the offending key. If
**every** retrieval source is unavailable the endpoint responds
`503 Service Unavailable + Retry-After: 5` (FR-011); a per-section
backend outage instead surfaces in-band as
`sections[*].status = "degraded"`.

### Decay-config tuning recipe

Each fact type has its own decay constant λ, applied as
`decay_weight = exp(-λ · age_seconds)`. Defaults (suitable for a
new install) live in `klams_types::DecayConfig::default`:

| FactType   | default λ | half-life     |
|------------|-----------|---------------|
| `TaskFact` | `1e-6`    | ≈ 8.0 days    |
| `UserFact` | `1e-9`    | ≈ 22.0 years  |
| `EnvFact`  | `1e-9`    | ≈ 22.0 years  |

To rebalance recall against staleness, override under `[decay.lambda]`
in `klams.toml`:

```toml
[decay]
task_interval_seconds = 60       # decay sweep cadence
batch_size            = 256

[decay.lambda]
TaskFact = 2e-6                  # halve the half-life of task chatter (~4d)
UserFact = 5e-10                 # double the half-life of user facts  (~44y)
EnvFact  = 1e-9                  # leave defaults
```

`klams-service` validates the config **before** binding the listener:

- Non-finite, negative, or unknown-`FactType` λ → exit code `2`
  with the offending key in stderr.
- `task_interval_seconds == 0` or `batch_size == 0` → exit code `2`.
- On success: one `INFO` line `decay config loaded: task_fact_lambda=…`
  is emitted and `klams_decay_config_reload_total` increments.

There is **no** SIGHUP-style hot-reload; restart the service to
apply a new `[decay]` block.

### Summarization knobs

`[summarization]` controls the background task that maintains the
`summaries` table (migration `0004`). Used by the events section of
`/memory/context` when raw rows would blow the token budget.

```toml
[summarization]
enabled              = true
task_interval        = "60s"   # also accepts "5m", "1h"
event_cluster_min    = 3       # only summarize ≥ N events
llm_fallback         = true    # try the chat LLM; on failure use extractive
llm_url              = "http://kubs0:11434/v1"   # OpenAI-compat base incl. /v1
llm_model            = "phi3:medium"
```

Sprint 014: the chat endpoint speaks the OpenAI-compatible dialect
(`GET {llm_url}/models` probe, `POST {llm_url}/chat/completions`), so
`llm_url` works with Ollama's `/v1` route, vLLM, or kvllm on kai; set
`llm_api_key` if the endpoint requires a bearer key. The legacy
`ollama_url` / `ollama_model` keys still parse as aliases — but note
the URL must now include the `/v1` segment.

When the chat endpoint is unreachable, the task records `mechanism = "extractive"`
and `klams_summarization_runs_total{mechanism="extractive"}` increments;
the events section in `/memory/context` keeps shipping headlines
("3x compile, 2x test"). Watch `klams_summarization_lag_seconds` for
the wall-clock age of the most recent successful cycle.

### Viewport: Context Preview pane

The viewport gains a `/preview` route backed by
`viewport/src/lib/components/ContextPreview.svelte`. The pane has a
query box, a 250 ms-debounced token-budget slider, a
raw-vs-summarized toggle, and per-section token-count readouts.
Each interaction calls `POST /memory/context` via the typed client
in `viewport/src/lib/api/context.ts` and renders the returned bundle
section-by-section, surfacing `sections[*].status` (`degraded`,
etc.) and any `Retry-After` hint on 503.

## Sprint 006 — Maintenance window + backups

This sprint adds an in-process scheduler that takes a Postgres `pg_dump`
and a Qdrant snapshot once per UTC day, an axum middleware that gates
non-critical writes while a backup is in flight, and a generic
exec-with-JSON status hook so any external observer (kpidash, an SRE
script, ansible-k) can subscribe to the lifecycle. End-to-end walkthrough
lives at
[sprints/006-maintenance-and-backups/quickstart.md](../sprints/006-maintenance-and-backups/quickstart.md).

### `[backup]` config block

Match
[`deploy/config/klams.example.toml`](../deploy/config/klams.example.toml).
Disabled by default — set `enabled = true` to opt in:

```toml
[backup]
enabled              = true
backup_dir           = "/ai/klams/backups"         # written atomically: .partial -> rename
window_start_utc     = "07:00"                     # HH:MM UTC; no DST drift
daily_count          = 14                          # newest N distinct dates per kind
weekly_count         = 4                           # newest N Sundays per kind
same_day_strategy    = "suffix"                    # "suffix" (default) | "overwrite"
status_hook          = "/usr/local/bin/klams-backup-shim"   # optional
status_hook_timeout  = "10s"                       # bounded; misbehaving hook can't stall a backup
```

Validate the config without starting the service:

```bash
just backup-validate-config
# OK: [backup] enabled=true backup_dir=/ai/klams/backups window_start_utc=07:00 ...
```

### `just` recipe additions

| Recipe                    | What it does |
|---------------------------|--------------|
| `backup-once`             | Runs `klams-service --run-backup-now` once: skips the scheduler but exercises every other path (maintenance flag, hook invocations, retention, metrics). |
| `restore-from <date> [--force]` | Restores `postgres-<date>.dump` + `qdrant-<date>.snapshot` from `[backup].backup_dir`. Refuses non-empty targets without `--force`. |
| `backup-validate-config`  | Loads `klams.toml`, runs `BackupConfig::validate()`, exits `0` on OK / `2` on validation error. |
| `backup-verify [<date>]`  | Read-only integrity check of a committed pair (default: today UTC). Runs `pg_restore --list` on the dump and `tar tf` on the snapshot, asserts both produce a non-empty listing. Exits `0` on OK / `1` on missing or unreadable artifact. |
| `backup-size`             | Brings up the test stack, loads the scale fixture, times one `run_once`, prints `kind | bytes | seconds`, and appends a dated entry to `sprints/006-maintenance-and-backups/sizing.md`. |

### Maintenance-mode error envelope

While `MaintenanceState::active == true`, non-`GET` requests that are
**not** marked as critical writes are short-circuited by the
`maintenance_check` middleware. Clients should be prepared for:

```http
HTTP/1.1 503 Service Unavailable
Retry-After: 30
Content-Type: application/json

{
  "error": "maintenance_window_active",
  "retry_after_seconds": 30
}
```

`retry_after_seconds` is the estimated number of seconds until the
running backup completes (computed from `RunningSnapshot.expected_end_at`)
with a 30-second floor. Reads (`GET /memory/*`) and User-source dissent
resolution (`POST /memory/dissents/{id}/promote|discard`) pass through
unchanged — see
[sprints/006-maintenance-and-backups/quickstart.md §3](../sprints/006-maintenance-and-backups/quickstart.md#3-observe-maintenance-mode-behavior).

### `/healthz` extension

`HealthSnapshot` gains a `maintenance` block reflecting the live
`MaintenanceState`:

```jsonc
{
  "status": "Ok",
  // ... existing subsystems ...
  "maintenance": {
    "active": true,
    "run_id": "01HZ0Q3X4WM7Q3X4WM7Q3X4WM7",
    "started_at": "2026-05-22T07:00:00Z",
    "expected_end_at": "2026-05-22T07:08:00Z"
  }
}
```

When no backup is in flight the block collapses to
`{ "active": false }` with the optional fields omitted.

### Status hook contract

Set `[backup].status_hook` to an executable path and klams will spawn
it three times per run — `event ∈ {started, finished, failed}` — with
a versioned JSON payload streamed to its stdin and the run's
identifiers exposed via environment variables (`KLAMS_BACKUP_RUN_ID`,
`KLAMS_BACKUP_EVENT`). The schema is the single source of truth:
[`contracts/backup-status-hook.schema.json`](../sprints/006-maintenance-and-backups/contracts/backup-status-hook.schema.json).

```jsonc
// finished example (lifecycle events match the schema's examples[])
{
  "schema_version": 1,
  "run_id":     "01HZ0Q3X4WM7Q3X4WM7Q3X4WM7",
  "event":      "finished",
  "started_at": "2026-05-22T07:00:00Z",
  "ended_at":   "2026-05-22T07:08:00Z",
  "duration_ms": 480000,
  "artifacts": [
    {"kind": "postgres", "path": "/ai/klams/backups/postgres-2026-05-22.dump",     "bytes": 12345678},
    {"kind": "qdrant",   "path": "/ai/klams/backups/qdrant-2026-05-22.snapshot",   "bytes": 9876543}
  ],
  "ok": true,
  "error": null
}
```

Hook invocations are bounded by `status_hook_timeout` (default `10s`)
with a 2-second SIGTERM grace before SIGKILL. **Hook failure is
observability, not control flow** — a missing executable, infinite
loop, or non-zero exit increments
`klams_backup_hook_invocations_total{ok="false"}` but never affects
the backup outcome. A misconfigured hook (path does not exist or is
not executable) is also a **non-fatal startup warning** — the
service logs a `WARN` line on boot and the backup task carries on,
recording the hook failure via the same `ok="false"` counter when
the run fires. This is the contract that the kpidash widget shim
consumes: subscribe to the lifecycle events from a tiny shell
script that publishes them on Redis (or any other transport), keep
the script under the timeout, and the live dashboard reflects backup
state within the SC-004 2-second budget.

### Verifying a committed backup

`just backup-verify [<date>]` runs a read-only integrity check
against the most recent (or explicitly-dated) pair in
`[backup].backup_dir`:

```bash
just backup-verify              # today UTC
just backup-verify 2026-05-24   # explicit date
# ==> postgres: /ai/klams/backups/postgres-2026-05-24.dump
#   bytes=21168 toc_entries=43 OK
# ==> qdrant:   /ai/klams/backups/qdrant-2026-05-24.snapshot
#   bytes=712192 tar_members=18 OK
# ==> backup-verify: OK
```

What it actually checks:

- **Postgres dump** — `pg_restore --list <file>` parses the dump's
  table of contents without touching a database. A truncated or
  corrupt dump fails to produce TOC entries. (Uses
  `[backup].pg_bin_dir/pg_restore` when set, else the `pg_restore`
  on `$PATH`.)
- **Qdrant snapshot** — qdrant snapshots are uncompressed tar
  archives; `tar tf <file>` listing succeeds with a non-zero member
  count on an intact snapshot.

For the strongest possible verification — exercising the same
`restore::run_from` code path the once-exercised drill validates —
follow the manual procedure in
[sprints/006-maintenance-and-backups/quickstart.md §5](../sprints/006-maintenance-and-backups/quickstart.md#5-restore-from-a-snapshot-fr-016)
against a throwaway compose stack and compare row counts.

## Sprint 007 — MCP server

Sprint 007 mounts a Model Context Protocol (MCP) surface on
`klams-service` so editors and shell agents can read and write
klams memories through a uniform tool interface. The MCP server is
**additive**: every existing REST endpoint, the viewport, and the
non-agentic writers (`klams-scanner`, `klams-monitor`) keep working
unchanged. Detailed contracts live under
[sprints/007-mcp-server/contracts/](../sprints/007-mcp-server/contracts/).

### Tool surface

| Tool | Scope | Purpose |
|------|-------|---------|
| `register_author` | `read` | Issue / refresh the caller's author id. Optional since sprint 018: write tools default to the bearer token's bound author; call this only to write as a separate per-session identity. `repo` accepts an absolute path or a bare repo name. |
| `memory_search` | `read` | Hybrid retrieval over facts + knowledge + events. |
| `event_search` | `read` | Filter `events` by category / task / time window / payload substring. Pure SQL — never hits the embedder (FR-004). |
| `memory_related` | `read` | Neighborhood expansion around a known memory id. |
| `dissent_propose` | `write` | File a dissent against a live canonical fact (sprint 015); lands as a pending `AgentProposal` for human review in the viewport. |
| `memory_add` | `write` | Add a fact or knowledge item. Same dedupe + dissent rules as REST. Flat input schema since sprint 018: `kind` (`"fact"` \| `"knowledge"`) discriminates, with `fact_type`+`payload` required for facts and `text` (+ optional `tags`/`source_path`/`repo`) for knowledge — no top-level `oneOf`, so Anthropic-bound agents can carry the tool. |
| `memory_append_event` | `write` | Append an event. Always canonical, never soft-deleted. |
| `memory_delete` | `write` | Soft-delete the caller's own fact / knowledge item by id. |
| `memory_admin_restore` | `admin` | Reverse a prior soft delete. |
| `memory_admin_hard_delete` | `admin` | Permanently remove a soft-deleted row. |
| `memory_admin_list_deleted` | `admin` | Page through soft-deleted rows for triage. |

The advertised tool list is filtered per-request by the bearer
token's grant; an `admin`-less token never sees the `memory_admin_*`
tools (FR-020). (Token-budgeted context bundles remain REST-only at
`/memory/context` — the `memory_context` tool sketched in 007 was
never mounted.)

Since sprint 018 the write tools' `author_id` argument is optional:
when omitted, the write is attributed to the author bound to the
caller's bearer token (`agent_name` in `[[auth.tokens]]`, or the
seeded `system` author for unbound/legacy tokens). Passing an
explicit `author_id` still works and always wins. Session teardown
also answers 204 (not 202) so mcp python-sdk clients no longer log
`Session termination failed: 202` on close.

### Scope configuration

Sprint 007 keeps the legacy single `auth.bearer_token` field (which
materializes into one grant with all three scopes) and adds an
`[[auth.tokens]]` array for fine-grained tokens:

```toml
[auth]
# Legacy single-token form — still supported, grants all scopes:
# bearer_token = "..."

[[auth.tokens]]
token = "viewport-readonly-XXXXXXXXXXXX"
scopes = ["read"]
label = "viewport"

[[auth.tokens]]
token = "ghcp-write-XXXXXXXXXXXXXXXX"
scopes = ["read", "write"]
label = "ghcp"

[[auth.tokens]]
token = "ken-admin-XXXXXXXXXXXXXXXXXX"
scopes = ["read", "write", "admin"]
label = "ken-admin"
```

Validation rules (enforced at load):

- At least one of `bearer_token` (non-empty) or `tokens` (non-empty)
  must be present.
- Every token must be ≥ 16 characters (loose entropy floor — real
  entropy is the operator's responsibility).
- Every grant's `scopes` array must be non-empty.

The `label` is surfaced in logs and metrics (`klams_mcp_calls_total{token_label}`)
so a noisy or rogue client is easy to identify without leaking the
raw token. Recommended layout: one read-only token for the viewport
so a UI compromise cannot mutate state, one read+write token per
agent, and a single admin token used only from your own shell.

### Soft-delete safety model

Facts and knowledge items support **soft delete** via `memory_delete`:

- `memory_delete` is idempotent — repeated calls on the same id are
  no-ops once the row is tombstoned (FR-014).
- A soft-deleted row vanishes from every read path (`memory_search`,
  `memory_related`, `/memory/context`, `GET /v1/authors/{id}/memories?state=live`)
  but is preserved in Postgres / Qdrant with `deleted_at` and
  `deleted_by_author_id` populated.
- Recovery is `memory_admin_restore`; the original delete
  attribution is preserved.
- `memory_admin_hard_delete` is the only path that removes the row
  permanently and is irreversible.
- Events are **never** soft-deleted (FR-015). They are append-only
  and have no `deleted_at` column.

This means a rogue write+scope agent can hide memories but cannot
destroy them. The DR drill at
[sprints/007-mcp-server/quickstart.md §12](../sprints/007-mcp-server/quickstart.md#12-restore-from-rogue-agent-drill)
walks through detecting and reversing a mass-delete.

### Viewport `/authors` review workflow

The viewport gains an `Authors` nav entry that drills into per-agent
state:

1. **`/authors`** — table of every registered author with rolling
   counts (writes, knowledge, events, soft-deletes, restores
   received). Filter by `agent_name` substring or `since` timestamp.
2. **`/authors/{id}`** — author header (model, repo, client app,
   first/last seen, counts) plus a memories table. Each row carries
   a state badge (`live` | `soft-deleted` | `hard-deleted`),
   a kind column, a summary, the updated-at timestamp, and a link
   that lands on `/facts/{id}`, `/knowledge/{id}`, or
   `/events/{id}` so you can pivot into the existing per-kind
   inspectors (FR-025).

> **Sprint 010 (US4):** both surfaces now render `counts.knowledge`
> as a distinct **Knowledge** measure alongside **Writes** (facts).
> They are shown separately, never summed — so an author that has only
> indexed knowledge (e.g. `klams-scanner`, `writes=0`) shows its real
> knowledge count instead of `0`. The two pages share one render helper
> (`viewport/src/routes/authors/counts.ts`).

Filter controls on the detail page: kinds checkboxes
(`fact` / `knowledge` / `event`) and a state selector
(`live` / `deleted` / `all`). Pagination uses the same cursor
contract as the REST endpoint.

Use it for incident review (who wrote this?), routine hygiene
(this scanner has been hammering the dissent queue), and the
post-restore audit (every restore drill should end with a glance
at the rogue author's `/authors/{id}` page to confirm counts
returned to baseline).

## Sprint 008 — Activity tab and `event_search`

Sprint 008 (`sprints/008-activity-observability/`) adds a single
cross-author **Activity tab** to the viewport and a matching MCP
tool (`event_search`) for cheap event lookup, both backed by the
same shared store query (R-001 — "two surfaces, one query").

### Viewport Activity tab (`/activity`)

Open the viewport and pick **Activity** from the nav. The tab
defaults to the last 24 hours across all kinds and all authors.

Filters:

- **From / To** — local-time pickers; sent to the service as ISO
  8601. The service rejects windows wider than 30 days with HTTP
  400 `WINDOW_TOO_LARGE`.
- **Kinds** — independent toggles for `fact`, `knowledge`, `event`.
- **State** — `live` (default), `soft-deleted`, or `all`. Soft-
  deleted rows render with an amber badge and keep their original
  `deleted_at` / `deleted_by` metadata (FR-015a); the row link
  still routes to the per-kind detail page.
- **Authors** — multi-select sourced from
  `GET /v1/authors`. Empty selection means "all authors".
- **Limit** — page size, 1–200.

The grid uses cursor pagination — when the service returns a
`next_cursor`, a **Load more** button appears beneath the table.

Unlike the per-author `/authors/{id}` view added in sprint 007,
the Activity tab is cross-author: the natural starting point for
"what happened in the last hour, across every agent and every
kind?"

### `event_search` MCP tool

`event_search` is the agent-facing counterpart to the Activity
tab. Read scope. Pure SQL — it never invokes the embedder, so
counters like `klams_tei_requests_total` must not increment for
a search-only workload (FR-004).

Arguments:

| Field | Type | Notes |
|-------|------|-------|
| `category` | string, optional | Exact match against `events.category`. |
| `task_id` | string, optional | Exact match against `events.payload->>'task_id'`. |
| `since` | RFC 3339 timestamp, optional | Lower bound on `occurred_at`. Defaults to 24 hours ago. |
| `until` | RFC 3339 timestamp, optional | Upper bound on `occurred_at`. Defaults to now. |
| `payload_match` | string, optional | Case-insensitive substring match against the serialized payload. |
| `limit` | u32, optional | 1–200, default 50. |
| `cursor` | opaque string, optional | Returned by a prior call's `next_cursor`. |

Use it for incident timelines (`category=Deploy`,
`since=2026-05-26T00:00:00Z`), task drill-downs
(`task_id=...`), and routine "what did this agent do?" queries.
The window cap is the same 30-day limit as the HTTP surface.

### Operator surface — `GET /v1/memories`

The HTTP route behind the Activity tab; read scope. Identical
filters and response shape to the Tauri wrapper, with the same
30-day window cap and the same cursor contract. Useful when you
want to script the same listing from the shell:

```bash
curl -sS -H "Authorization: Bearer $KLAMS_READ_TOKEN" \
  "http://kubs0:7777/v1/memories?kinds=event&state=deleted&limit=50" | jq
```

### Performance baseline

Run `just bench-seed && just bench-run` (see
[tools/bench/README.md](../tools/bench/README.md)) to refresh
[sprints/008-activity-observability/perf-baseline.md](../sprints/008-activity-observability/perf-baseline.md).
Per FR-022, the harness always exits 0 — the baseline is a
measurement, not a CI gate.

## Sprint 009 — Stability, attribution, and bench-clean

Sprint 009 (`sprints/009-stability-attribution/`) adds three operator
recipes: a loopback soak to validate FR-001/SC-001, an
author-based bench-clean, and a one-shot re-attribution CLI for
historical REST writes.

### `just soak` — loopback CLOSE_WAIT regression harness

[`tools/soak/`](../tools/soak/) drives sustained loopback traffic at
the live service and samples the host's file-descriptor and
`CLOSE_WAIT` counts at the start and end of the window. The 18-hour
run is the SC-001 regression for kwi #26:

```bash
just soak --duration 18h 2>&1 | tee /tmp/soak-018h.log
```

Shorter windows are useful for smoke checks (`--duration 15m`).
Record the start/end fd/CLOSE_WAIT samples and the SC-001 verdict
in
[sprints/009-stability-attribution/soak-report.md](../sprints/009-stability-attribution/soak-report.md).
The harness exits 0 regardless of outcome — like `bench-run`, it is
a measurement, not a gate.

### Author-based `just bench-clean`

With sprint 009's attribution wiring landed, the bench seeder writes
as its own `agent_name = "klams-bench"` author (configured in
`klams.toml`; see [setup.md](setup.md#sprint-009--stability--attribution)).
`just bench-clean` is now a single author-scoped purge instead of
the payload-filter walk it used in sprint 008:

```bash
# Requires PGPASSWORD; reads PGHOST/PGUSER/PGDATABASE/QDRANT_URL
# from env (defaults: 127.0.0.1, klams, klams, http://127.0.0.1:6333).
export PGPASSWORD=<from /ai/klams/config/klams.toml [postgres].url>
just bench-clean
```

It deletes every row where `author_id = (SELECT id FROM authors
WHERE agent_name = 'klams-bench')` from `facts`/`events`, and every
Qdrant point whose `author_id` payload matches the same UUID. No-op
when the `klams-bench` author has no rows.

### `reattribute-system` — one-shot historical repair

For deployments with REST writes from before sprint 009 (all
stamped as `system`), the standalone
[`tools/reattribute-system/`](../tools/reattribute-system/) CLI
reassigns each row to the author of the `register_author` event
that immediately preceded the write. Rows with no resolvable
antecedent land on the seeded `lost-author` identity rather than
staying on `system`.

```bash
# Dry-run — prints per-author reassignment counts plus the
# `lost-author` bucket without touching the store:
cargo run --release -p reattribute-system

# Commit:
cargo run --release -p reattribute-system -- --apply
```

The repair is idempotent (a second `--apply` is a no-op) and is
intended as a one-time cutover step. Setup details and the
`agent_name` token config live in
[setup.md § Sprint 009](setup.md#sprint-009--stability--attribution).

## Sprint 021 — corpus hygiene + miss log

### Scanner: delete-before-reindex + file-type allowlist

Two ingestion-quality fixes landed in the scanner:

- **Edited files no longer leak stale chunks.** When a tracked file's
  content changes, `scan_root` now deletes its previous knowledge points
  (`POST /memory/knowledge/delete?source_file=<abs>`) *before*
  publishing the new chunks. Before this, an edit added new points but
  left the old versions live and searchable — the corpus re-polluted
  itself on every edit. If the delete fails the file's cursor is left
  unadvanced so the next scan retries rather than stacking new chunks on
  stale ones.
- **Only content is indexed.** The walker now applies a file-type
  allowlist (source, docs/prose, config prose; extensionless ops files
  like `Dockerfile`/`Makefile`/`justfile`) and explicitly drops
  lockfiles, JSON fixtures, SVGs, images, and archives — see
  `ALLOW_EXT` / `ALLOW_NAMES` / `DENY_NAMES` in
  `crates/klams-scanner/src/walk.rs`. A missing extension is recoverable
  (add it; the miss log surfaces demand); a false positive costs tokens
  on every retrieval.

### One-time stale-chunk purge (operator step)

The delete-before-reindex fix stops *future* leaks; chunks orphaned by
edits made before it deployed remain until each file next changes. To
purge them in one pass after deploying the new scanner, clear the
scanner cursor and rescan — every file is then treated as changed, so
each runs delete-before-reindex and re-publishes its current chunks:

```sh
sudo systemctl stop klams-scanner.timer klams-scanner.service
sudo rm -f /var/lib/klams/scanner.sqlite   # the cursor DB (see monitor.toml/scanner config for the path)
just scanner-once                           # full rescan: delete-then-reindex per file
sudo systemctl start klams-scanner.timer
```

This re-embeds the whole corpus, so it is the same cost as a full
re-index — if sprint 022's re-index is imminent, fold the purge into
that rather than paying it twice.

### Miss log

`memory_search` now records misses — calls that returned nothing
(`reason=zero_hit`) or only a weak knowledge match
(`reason=low_score`, top Qdrant cosine < 0.5) — two ways:

- **Metric:** `klams_search_misses_total{reason}` drives the Grafana
  **"Search miss rate (zero-hit / low-score)"** panel.
- **Durable row:** a `search_miss` table (migration `0010`) captures the
  query text, caller (bearer `agent_name`), reason, top score, hit
  count, and kinds queried — the "what did an agent want and not get"
  record that drives chunking fixes (022) and the lexical-search
  decision (024). The insert is fire-and-forget: a failure is logged at
  debug and never affects the live search. The table is append-only;
  prune old rows by `created_at` as an operator concern.

```sql
-- Recent misses, newest first:
SELECT created_at, caller, reason, top_score, hit_count, query
FROM search_miss ORDER BY created_at DESC LIMIT 50;

-- What are agents asking for and getting nothing?
SELECT query, count(*) FROM search_miss
WHERE reason = 'zero_hit' GROUP BY query ORDER BY count(*) DESC;
```
