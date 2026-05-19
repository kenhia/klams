# Phase 1 — Data Model: Non-Agentic Writes, Integrations, and the Systemd Switchover

**Sprint**: 003-non-agentic-writes
**Date**: 2026-05-18
**Companion**: [plan.md](plan.md), [research.md](research.md), [contracts/](contracts/)

This document is normative for the entity shapes added or constrained
by sprint 003. Items not mentioned here keep their Phase 1/2
definitions unchanged. Validation rules below are enforced by
`klams-core::validate::*` and surfaced through the existing 422
response shape.

---

## 1. AnsibleFactWrite (no new DB shape)

A logical role, not a new table. An `UserFactWrite` or `EnvFactWrite`
becomes an "Ansible-originated fact" iff it arrives with
`source = "Task"` **and** carries a `task_id` field in the payload
matching the Ansible run-id shape.

### Payload constraints (additive over Phase 2 validators)

| Field      | Type     | Required | Constraint |
|------------|----------|----------|------------|
| `task_id`  | string   | yes (when `source=Task` from Ansible) | matches `^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$` (UUID v1-5) OR a 32-char `ansible-` prefixed run-id. Length ≤ 64. |
| `host`     | string   | yes (EnvFact); optional (UserFact) | matches existing hostname-shape rule from Phase 2 (`^[a-z][a-z0-9-]*$`, length 1..=64). |

### Validation rules

- If `source = "Task"` **and** the payload includes `task_id`, the
  fact is treated as a trusted non-agentic write. Dispatcher
  short-circuits the dissent comparison against same-or-lower-trust
  rows (an existing `Task` row is replaced; an existing `User` row
  diverts the new write to `dissents`).
- A `Task`-source write **without** `task_id` is still accepted
  (existing Phase 1 controller traces) — the `task_id` requirement
  applies only when the caller declares itself as Ansible by setting
  `payload.task_id`. There is no separate "I am Ansible" header.
- Canonical hash (existing Phase 1 mechanism) includes `task_id`
  only when present; this ensures a single play re-running produces
  zero new versions (FR-003).

### Rationale

Distinguishing Ansible writes from other `Task`-source writes by
payload shape (rather than a new endpoint or a new source) keeps the
existing four-value `source` enum stable and lets the dispatcher
remain one switch statement.

---

## 2. ScannedKnowledgeChunk

The payload shape produced by `klams-scanner` for every chunk it
posts to `POST /memory/knowledge/index`. The existing Phase 1
endpoint accepts arbitrary JSON payloads; this sprint pins the
fields the scanner emits so the data is queryable by metadata.

### Payload fields

| Field           | Type     | Required | Constraint |
|-----------------|----------|----------|------------|
| `source_file`   | string   | yes      | absolute path; length ≤ 4096. |
| `repo`          | string   | yes      | the top-level directory name under the configured scanner root (e.g. `klams`, `ansible-k`, `obsidian-gratch`); length 1..=128. |
| `chunk_index`   | integer  | yes      | 0-based ordinal of the chunk within the file. |
| `content_hash`  | string   | yes      | lowercase hex sha256 of the post-normalized chunk text. Length = 64. |
| `mtime_ns`      | integer  | yes      | file mtime at scan time, in nanoseconds since epoch. |
| `chunk_text`    | string   | yes      | the text indexed; length 1..=8192. |

### Indexing

- `(source_file, content_hash)` is the dedupe key inside Qdrant
  (existing knowledge_items mechanism).
- On file deletion, the scanner posts a synthetic
  `POST /memory/knowledge/delete?source_file=<abs_path>` request to
  the endpoint added in this sprint (tasks T010b). Phase 1 did not
  ship a delete path; T010b adds it as part of the foundational
  contract surface, not as scanner-internal code.

### Rationale

Pinning a small set of metadata fields makes the
`/memory/search?filter.repo=klams` workflow possible without
indexing JSON-blob payloads more deeply than Phase 1 already does.

---

## 3. ServiceEvent (constrained Phase 1 envelope)

An `Event` row with `category = "Service"` and the constrained payload
below. The existing `events` table schema is unchanged.

### Payload fields

| Field      | Type     | Required | Constraint |
|------------|----------|----------|------------|
| `service`  | string   | yes      | length 1..=128. |
| `event`    | string   | yes      | one of `up`, `down`, `restart`, `version_changed`. |
| `version`  | string   | optional | length ≤ 64 (only meaningful for `version_changed`). |
| `port`     | integer  | optional | 1..=65535. |
| `host`     | string   | yes      | hostname-shape rule from Phase 2. Required so multi-host monitor deployments can distinguish same-named units (per FR-009). |

### Validation

Added to `klams-core::validate::events` as
`ServiceEventValidator`. Rejects unknown `event` values with
`422 payload.event not in [up, down, restart, version_changed]`.

---

## 4. ExecutionTraceEvent

An `Event` row with `category = "Execution"`. Unchanged envelope;
constrained payload below.

### Payload fields

| Field      | Type     | Required | Constraint |
|------------|----------|----------|------------|
| `task_id`  | string   | yes      | same shape as in §1 (UUID or `ansible-`/controller-prefixed run-id). |
| `tool`     | string   | yes      | length 1..=128. |
| `phase`    | string   | yes      | one of `started`, `completed`, `failed`. |
| `detail`   | string   | optional | length ≤ 4096. |

### Indexing (DB)

New migration `0003_events_task_idx.sql`:

```sql
-- Up
-- Run outside a transaction (the migrator MUST detect the
-- `-- @no-transaction` directive on the first line and skip BEGIN/COMMIT).
-- @no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS events_task_id_created_at_idx
  ON events ((payload->>'task_id'), created_at)
  WHERE category IN ('Execution', 'Service');

-- Down
-- @no-transaction
DROP INDEX CONCURRENTLY IF EXISTS events_task_id_created_at_idx;
```

The expression-index path is taken (rather than promoting
`task_id` to a column) to keep this sprint's migration
purely additive — no row rewrites, no `events` shape change.
CONCURRENTLY is used so the index build does not block writes
on a non-trivial `events` table.

### Validation

`ExecutionTraceEventValidator` enforces the table above. Unknown
`phase` values rejected as `422 payload.phase not in [started,
completed, failed]`.

---

## 5. PolicyTable

The in-memory Rust struct backing `GET /memory/policy`.

```rust
// crates/klams-core/src/policy.rs
#[derive(Debug, Clone, serde::Serialize)]
pub struct PolicyEntry {
    pub rank: u8,           // higher = more trusted
    pub description: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PolicyTable {
    pub user: PolicyEntry,           // serializes as "User"
    pub controller: PolicyEntry,     // serializes as "Controller"
    pub task: PolicyEntry,           // serializes as "Task"
    pub agent_proposal: PolicyEntry, // serializes as "AgentProposal"
}
```

Default (production) values:

| Source         | rank | description |
|----------------|------|-------------|
| `User`         | 4    | Direct user input via viewport or CLI; wins all contradictions. |
| `Controller`   | 3    | Controller process on a trusted homelab machine; wins over Task and below. |
| `Task`         | 2    | Ansible plays, scanner, monitors, controller execution traces. |
| `AgentProposal`| 1    | Agent-originated writes; diverted to dissents when they contradict any higher-trust row. |

The dispatcher's switch is generated from this struct (no
hand-maintained duplication; verified by the contract test in
[contracts/memory_policy.md](contracts/memory_policy.md)).

---

## 6. Scanner cursor (local sqlite)

Single table, single host, owned by the scanner. **Not** in Postgres.

### Path

```text
${XDG_STATE_HOME:-$HOME/.local/state}/klams/scanner.sqlite
```

Under systemd, `StateDirectory=klams` places the file at
`/var/lib/klams/scanner.sqlite` and ensures the `klams` system user
owns it (mode 0700).

### Schema

```sql
CREATE TABLE IF NOT EXISTS file_cursor (
    absolute_path     TEXT PRIMARY KEY,
    content_hash      TEXT NOT NULL,        -- sha256 of full file post-normalization
    mtime_ns          INTEGER NOT NULL,
    last_indexed_at   INTEGER NOT NULL      -- unix seconds
);

CREATE INDEX IF NOT EXISTS file_cursor_last_indexed_idx
  ON file_cursor (last_indexed_at);
```

### Operations

- **Upsert on successful index**: `INSERT … ON CONFLICT(absolute_path) DO UPDATE`.
- **Diff on walk**: a file is re-embedded iff `content_hash` differs
  from the stored value (or no row exists). `mtime_ns` is a cheap
  pre-filter — if mtime matches the stored value, skip the hash
  computation.
- **Delete on missing**: at the end of each walk, any row whose
  `absolute_path` was not seen during the walk is deleted, and a
  `POST /memory/knowledge/delete?source_file=…` request is sent.
- **Resume**: every successful per-file index is its own transaction.
  A mid-walk crash loses no progress; the next scan picks up where
  the previous one stopped (FR-005, US2 AS3).

---

## 7. State transitions summary

The only state machine new this sprint lives inside the monitor's
in-memory cache, not in the DB:

```text
unit-state previous   poll result   event posted
-------------------   ----------    -------------
(absent)              active        service.up
(absent)              inactive      service.down
active                inactive      service.down
inactive              active        service.up
active                active        (none)
inactive              inactive      (none)
active|inactive       version-diff  service.version_changed
```

`service.restart` is **not** synthesized by the poller (we can't
distinguish a fast restart from a steady-state poll within 30s
granularity). External callers (e.g. an Ansible play that restarts
the unit on purpose) MAY post `service.restart` directly.

---

## 8. Migration footprint

Total schema delta this sprint:

| Change | Type | Reversible | Reason |
|--------|------|-----------|--------|
| `events_task_id_created_at_idx` | additive index | YES (DROP INDEX) | FR-010 — sub-linear per-task trace queries. |

No table additions, no column additions, no row rewrites.
