# Implementation Plan: Stability & Attribution

**Branch**: `009-stability-attribution` | **Date**: 2026-05-27 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `sprints/009-stability-attribution/spec.md`

## Summary

Three P1 defects ship together: (a) kwi #26 — klams-service wedges
when loopback `CLOSE_WAIT` sockets exhaust its fd cap; (b) the REST
write surface attributes every memory to the seeded `system` author,
ignoring the bearer token's identity; (c) the historical store needs
a one-shot repair after (b) lands so existing per-author surfaces stop
under-reporting real agents. Two P2 stories ride along: refresh the
sprint 008 perf baseline (blocked while #26 was open) and fix kwi #28
(viewport Authors → memory summary click → 404). One P3 cleanup
tightens Phase 6 MCP test isolation.

Technical approach: bind per-connection timeouts and a per-peer
concurrency cap into the existing axum/hyper stack; extend
`TokenGrantConfig` with an optional `agent_name`, resolve it to an
`author_id` at service startup, plumb that id through the
`MemoryWrite` job variants, and route the worker to the existing
`*_with_author` store paths (adding `index_knowledge_with_author` for
Qdrant payload stamping). Ship the repair as an admin-only operation
in `klams-store` driven from a CLI subcommand. Reuse the sprint 008
`hrefFor()` helper for the viewport fix and randomize the Phase 6
Qdrant collection name per test.

## Technical Context

**Language/Version**: Rust 1.83 (workspace edition 2021); TypeScript
(SvelteKit) for the viewport; Tauri host crate for the desktop shell.
**Primary Dependencies**: axum 0.7 + hyper + tower for the HTTP
stack; `sqlx` for Postgres; `qdrant-client` for vector storage; `rmcp`
for the MCP surface; vitest + svelte-check for viewport tests.
**Storage**: Postgres 16 (facts, events, authors), Qdrant
(knowledge_items collection).
**Testing**: `cargo test` workspace + `just gate`; vitest for
viewport; `svelte-check` for type safety; the sprint 008
`tools/bench` harness for perf.
**Target Platform**: Linux server (`kubs0`) for service + Postgres +
Qdrant; Windows desktop for the viewport (Tauri build).
**Project Type**: Rust workspace with a SvelteKit/Tauri sibling
(`viewport/`), already established in sprints 001–008.
**Performance Goals**: Sustain the loopback half-close soak in
SC-001 (24h, bounded fd count); meet the existing perf-baseline
target (100 samples × 10 queries) once the rerun is unblocked. No
new latency budgets are introduced by this sprint.
**Constraints**: No new external services. Repair must run against
the live Postgres without long table locks (chunked updates). The
attribution wiring must keep current tokens (no `agent_name` set)
valid — they fall back to the documented `system` default.
**Scale/Scope**: ~9 Rust crates + viewport touched; ~5 new
migrations or schema-adjacent changes (auth resolution table,
pipeline struct fields); ~3 new contracts in `contracts/`.

## Constitution Check

*GATE: must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Compliance |
|-----------|-----------|
| I. Spec-Driven Development | Spec `spec.md` is in place with FRs/SCs; this plan and downstream artifacts go under the same feature directory. ✅ |
| II. Test-Driven Development | Every FR maps to either an existing test surface (contract tests under `crates/klams-api/tests/`, `crates/klams-service/tests/`, `viewport/src/routes/.../*.test.ts`) or a new one called out in Phase 1. Repair (FR-013…016) gets a dedicated `klams-store` test. Stability (FR-001…005) gets a soak harness binary under `tools/`. ✅ |
| III. Code Standards Gate | All changes must keep `just gate` green (fmt, clippy `-D warnings`, all tests). No new dependencies that would require constitution review. ✅ |
| IV. Documentation | Phase 1 produces `quickstart.md`; polish phase updates `docs/architecture.md`, `docs/setup.md`, `docs/usage.md` per the spec's deliverables. ✅ |
| V. Quality & Observability | Repair binary emits structured `tracing` output and counts (FR-015). Stability adds new structured logs on connection-timeout reaps. No raw stack traces. ✅ |
| VI. Simplicity & Intentional Design | No new external services or frameworks. Reuse existing `*_with_author` store paths (added in sprint 007), existing `hrefFor()` helper (added in sprint 008), existing bench infrastructure. No speculative interfaces. ✅ |

Gate: **pass**, no violations to justify.

## Project Structure

### Documentation (this feature)

```text
sprints/009-stability-attribution/
├── plan.md              # this file
├── research.md          # Phase 0 — decisions and tradeoffs
├── data-model.md        # Phase 1 — schema and config deltas
├── quickstart.md        # Phase 1 — operator walkthrough
├── contracts/
│   ├── README.md
│   ├── token-grant-config.md      # TokenGrantConfig.agent_name shape + startup resolution
│   ├── reattribution-cli.md       # one-shot repair CLI contract
│   └── connection-limits.md       # per-connection timeout + per-peer cap config
├── checklists/
│   └── requirements.md  # spec quality (already created)
└── tasks.md             # produced by /speckit.tasks
```

### Source Code (repository root)

```text
crates/
├── klams-api/                  # REST handlers: read author_id from request extension
│   └── src/handlers/{facts,events,knowledge}.rs
├── klams-core/                 # Worker: route MemoryWrite to *_with_author
│   └── src/worker.rs
├── klams-mcp/                  # No changes (MCP already correct)
├── klams-monitor/              # Optional: surface attributed-write rate
├── klams-service/              # Startup: resolve agent_name → author_id;
│   ├── src/main.rs             # apply connection timeouts + per-peer cap;
│   ├── src/config.rs           # TokenGrantConfig extension
│   └── tests/                  # Phase 6 test-isolation fix
├── klams-store/                # Repair function + tests; add index_knowledge_with_author
│   ├── src/postgres.rs
│   └── src/qdrant.rs
└── klams-types/                # TokenGrantConfig.agent_name; pipeline structs gain author_id
    ├── src/auth.rs
    ├── src/pipeline.rs
    └── src/requests.rs

tools/
├── bench/                      # Seeder rewritten to use klams-bench bearer; bench-clean SQL
├── soak/                       # NEW: half-close repro harness (Story 1)
└── reattribute-system/         # NEW: CLI binary that runs the repair (Story 3)

viewport/
├── src/routes/
│   ├── activity/row.ts         # hrefFor()/summaryFor() — reused unchanged
│   └── authors/[id]/+page.svelte  # call hrefFor() instead of bespoke link
└── src/routes/authors/[id]/row.test.ts  # NEW: vitest assertion

deploy/
└── systemd/
    └── klams-service.service   # NEW (or amended): LimitNOFILE=65536

docs/
├── architecture.md             # attribution section update; connection-limits note
├── setup.md                    # systemd LimitNOFILE; reattribution step
└── usage.md                    # bench-clean author flow; soak harness invocation

sprints/008-activity-observability/
└── perf-baseline.md            # refreshed by Story 5
```

**Structure Decision**: Reuse the existing Rust workspace + viewport
sibling. Add three new binaries under `tools/` (soak harness,
re-attribution CLI, and the existing bench tools stay where they are).
No new crates — every change lands in a crate that already exists, in
keeping with constitution principle VI.

## Phase 0 — Research

Deliverable: `research.md` (in this directory) covering:

1. **Connection lifecycle tuning** — axum / hyper /
   `tower::timeout::TimeoutLayer` settings that bound CLOSE_WAIT
   without breaking long-poll or SSE handlers (we have none today,
   so the simple case applies). Resolve "what timeout values?" and
   "concurrency cap per peer: where in the layer stack?".
2. **Author resolution at startup** — does `TokenGrantConfig`
   validation belong in `klams-types` (alongside the existing
   validators) or in `klams-service`? Decide whether resolution
   happens lazily (first request) or eagerly (startup) — recommend
   eager so misconfigurations fail loudly.
3. **Pipeline carrier shape** — extend `UpsertFact`, `AppendEvent`,
   and `IndexKnowledge` with `author_id: Uuid`, or pass it
   alongside via the `MemoryWrite` enum? Recommend on-struct so the
   field is always present and visible at the worker dispatch.
4. **Re-attribution algorithm** — which provenance signal in
   `events` is reliable enough to recover a fact's true author? The
   `events.author_id` column already exists; for each
   `system`-stamped fact, look for the most recent `fact_upsert`
   event referencing that fact's id with a non-system author.
   Decide tie-breaks and what counts as "unambiguous".
5. **Qdrant payload stamping** — current `index_knowledge` writes
   payloads without `author_id`; the MCP path already stamps via
   `index_knowledge_with_author`. Confirm that's the right place to
   hook and what the existing payload schema looks like.
6. **Phase 6 test isolation** — per-test random Qdrant collection
   name (cheap) vs. testcontainers (expensive). Lock in cheap.
7. **Viewport href unification** — confirm `hrefFor()` exported from
   `routes/activity/row.ts` covers every memory kind the Authors
   view exposes (fact, event, knowledge).

All decisions captured with Decision / Rationale / Alternatives.

## Phase 1 — Design & Contracts

### Deliverables

1. **`data-model.md`** — concrete deltas:
   - `TokenGrantConfig` gains `agent_name: Option<String>`; document
     validation (length, charset matching existing author rules) and
     the `system` fallback.
   - `UpsertFact`, `AppendEvent`, `IndexKnowledge` pipeline structs
     gain `author_id: Uuid`.
   - Qdrant payload schema gains `author_id` (string, lowercase
     UUID) and `author_agent_name` (string).
   - Re-attribution repair: input table (`facts`, `events`,
     `knowledge_items`), provenance lookup
     (`events WHERE category IN ('fact_upsert', 'event_append',
     'knowledge_index') AND author_id <> SYSTEM_AUTHOR_ID`),
     update target (`facts.author_id`, `events.author_id`,
     Qdrant payload).
2. **`contracts/token-grant-config.md`** — TOML config shape, the
   startup resolution algorithm, the validation errors raised on
   invalid `agent_name`.
3. **`contracts/reattribution-cli.md`** — CLI invocation
   (`reattribute-system --dry-run`, `--apply`), the output report
   shape (counts in FR-015), exit codes, idempotency guarantee.
4. **`contracts/connection-limits.md`** — per-connection
   `header_read_timeout`, `keep_alive_timeout`, per-peer concurrency
   cap, configuration surface (TOML keys), defaults, and the
   structured-log event names emitted on reap.
5. **`quickstart.md`** — operator walkthrough:
   - Configure a token with `agent_name`.
   - Write a fact via REST; confirm attribution via Activity tab.
   - Run the soak harness for a representative window; observe
     stable fd count.
   - Run `reattribute-system --dry-run`, inspect report, then
     `--apply`.
   - Refresh the perf baseline.
   - Click through Authors → Summary → details pane.
6. **Agent context update** — update
   `.github/copilot-instructions.md` SPECKIT block to point at this
   plan.

### Contracts

| Contract | Existing surface | New surface |
|----------|------------------|-------------|
| Token grant config | `crates/klams-types/src/auth.rs::TokenGrantConfig` | Adds `agent_name: Option<String>` field; documents `system` fallback and startup resolution |
| REST writes | `POST /v1/facts` / `events` / `knowledge` | Wire shape unchanged; semantics: row's `author_id` reflects bearer binding |
| MCP writes | `memory_add`, `memory_append_event` | Unchanged — already correct |
| Repair CLI | NEW: `tools/reattribute-system` binary | `--dry-run` / `--apply` with structured report |
| Connection limits | NEW: `[service.limits]` TOML section | Documented in `contracts/connection-limits.md` |
| Bench seeder | `tools/bench` | Now requires `klams-bench` bearer; `just bench-clean` becomes author-based |

### Post-design constitution re-check

Re-evaluate after writing the artifacts above. Same six principles;
expect to pass without violations because every change reuses an
existing crate or established pattern. If anything new emerges
(e.g. a new dependency for the connection-cap layer), record it in
the Complexity Tracking table.

## Phase 2 — Tasks (NOT in this command)

Produced by `/speckit.tasks` from this plan. Expected shape:

- **Phase 1** (US1 — Stability): connection-limit config + tower
  layer integration; soak harness binary; systemd unit update;
  fd-count assertions in CI-friendly smoke test.
- **Phase 2** (US2 — Attribution): `TokenGrantConfig.agent_name`;
  startup author resolution; pipeline struct fields; REST handler
  extension reads; worker dispatch updates; `index_knowledge_with_author`
  + Qdrant payload schema bump; bench seeder rewrite; contract tests.
- **Phase 3** (US3 — Repair): provenance-lookup function in
  `klams-store`; CLI binary; dry-run/apply modes; idempotency test;
  smoke test against live store.
- **Phase 4** (US5 prep — Bench cleanup parity): author-based
  `just bench-clean`; remove payload-pattern fallback.
- **Phase 5** (US5 — Perf rerun): execute, validate, commit
  refreshed `perf-baseline.md`.
- **Phase 6** (US6 — Test isolation): randomize Phase 6 Qdrant
  collection; assert no `--test-threads=1` dependency.
- **Phase 7** (US4 — Viewport): swap Authors-view link to
  `hrefFor()`; vitest assertion.
- **Polish**: docs updates (architecture/setup/usage), backlog
  archive moves, kwi #26 + #28 → closed.

## Complexity Tracking

No constitution violations. No entries required.
