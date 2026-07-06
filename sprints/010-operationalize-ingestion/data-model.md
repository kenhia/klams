# Phase 1 Data Model: Operationalize Ingestion

**Feature**: `010-operationalize-ingestion` | **Date**: 2026-06-09
**Plan**: [plan.md](plan.md) | **Research**: [research.md](research.md)

This sprint introduces **no database schema changes**. The "entities"
here are deployment configuration, the existing knowledge/event shapes
the ingestion path already writes, and the viewport count shape that
already exists end-to-end. They are documented so verification has a
fixed target.

---

## 1. Scanner config (`/etc/klams/scanner.toml`) — deployment artifact

Shape is the existing `Config` in `crates/klams-scanner/src/main.rs`
(unchanged). Documented here because the **deployed values** are the
load-bearing decision from [research.md](research.md) §R2.

| Field | Type | Deployed value on `kubs0` | Notes |
|-------|------|---------------------------|-------|
| `url` | string | `http://127.0.0.1:7777` | klams service base URL (loopback) |
| `token` | string | scoped scanner bearer | author identity for ingestion writes |
| `roots` | string[] | `["/home/ken/src", "/home/ken/obsidian"]` | **absolute**, not `~` (R2) |
| `interval_secs` | u64 | `3600` | overridden by the timer; unit runs `--once` |
| `state_dir` | string | aligns with `StateDirectory=klams` | SQLite cursor home; must resolve for `User=klams` |

**Validation rules** (verification, not new code):
- `roots` MUST be absolute and readable by the `klams` user under
  `ProtectHome=read-only`.
- `token` MUST resolve to a registered author so its knowledge writes are
  attributable (feeds US4's count).

## 2. Monitor config (`/etc/klams/monitor.toml`) — deployment artifact

Shape is the existing `Config` in `crates/klams-monitor/src/main.rs`
(`url`, `token`, `units`, `interval_secs`).

| Field | Type | Deployed value | Notes |
|-------|------|----------------|-------|
| `url` | string | `http://127.0.0.1:7777` | klams service base URL |
| `token` | string | scoped monitor bearer | author identity for `Service` events |
| `units` | string[] | the same units the python looper watches | parity basis (R4) |
| `interval_secs` | u64 | match the looper's poll cadence | so parity comparison is fair |

**Validation rule**: `units` MUST match the legacy looper's watched set
so the parity window (R4) is a like-for-like comparison.

## 3. Knowledge item (existing — written by the scanner)

No change. An embedded chunk with payload carrying at least
`source_file` (source attribution, FR-010), `author_id` /
`author_agent_name` (feeds US4 counts), and content hash (idempotency,
FR-009). Persisted in Qdrant `knowledge_items`; survives service restart
(FR-011).

## 4. Service-lifecycle event (existing — written by the monitor)

No change. A typed `Service` event with kind ∈ {Up, Down,
VersionChanged} per `crates/klams-monitor/src/state.rs`
(`ServiceEventKind`), the watched service name, and the transition. This
is the parity yardstick for retiring the looper (FR-012, FR-014).

## 5. Per-author counts (existing — end-to-end, render gap only)

No change to the data shape; documented because US4 closes the render
gap, not a data gap (see [research.md](research.md) §R1).

```
API  AuthorCounts { writes, knowledge, events, soft_deletes, restores_received }
       ▲ knowledge ← store AuthorWithCountsOut.writes_knowledge
       ▲ writes    ← Postgres fact_count
viewport types.ts AuthorCounts { writes, knowledge, events, soft_deletes, restores_received }
       ▲ knowledge field present, but NOT rendered by the Svelte pages
```

**Residual**: the Authors list + detail Svelte must display `knowledge`
alongside `writes` (US4 / FR-015–FR-016). Backend and TS types already
carry it.

---

## State transitions

The only meaningful state machine this sprint is the **monitor cutover**,
which is an operational sequence, not a data transition:

```text
looper-only ──install Rust monitor──▶ both-running (parity window)
                                         │ parity demonstrated (R4)
                                         ▼
                                   rust-monitor-only  ── steady state
```

No row-level state transitions are introduced.
