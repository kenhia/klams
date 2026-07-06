# Implementation Plan: Activity & Observability

**Branch**: `008-activity-observability` | **Date**: 2026-05-25 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/sprints/008-activity-observability/spec.md`

## Summary

Sprint 008 closes three usability gaps left behind by sprint 007 and stands up the perf measurement infrastructure that sprint 007's SC-006 assumed but never built. The work is intentionally additive — **no schema migrations**, no new runtime dependencies, no changes to the public `Memory` projection shape beyond optionally surfacing the existing `deleted_at` / `deleted_by_author_id` fields when soft-deleted rows are requested.

Five deliverables, all sitting on infrastructure shipped in sprint 007:

1. **`event_search` MCP tool** — a new read-classified tool in `klams-mcp` that returns events filtered by `author_id`, `category`, `since`, `until`, and `payload_match` (exact-equality JSON match on the event payload). Cursor pagination on `(created_at, id)` matches the existing `GET /v1/authors/{id}/memories` cursor shape. Pure SQL — no embedding pipeline.
2. **`GET /v1/memories` HTTP endpoint** — a new cross-author, all-kinds listing endpoint in `klams-api` that returns the public `Memory` projection paginated newest-first, filtered by `since`/`until`/`kinds`/`state`/`authors`. Bounded by a **30-day window cap** (FR-009) returning `400 WINDOW_TOO_LARGE` when exceeded. Same `read`-scope gate, same cursor encoding, same projection types as sprint 007.
3. **Viewport `/activity` route** — a new top-level Svelte route that wraps `GET /v1/memories` via a new Tauri command and TypeScript type, reusing the per-author drilldown row primitives. Default 24h window, all kinds, live state; controls for from/to/kind/state/authors and cursor-driven "next page".
4. **Grafana panel fix + checked-in Prometheus scrape config** — adds the missing "MCP author activity" panels to `deploy/grafana/klams.json` with PromQL that matches the labels actually emitted by `klams-mcp` (`agent_name`, `model`, plus `kind` on writes and `mode` on deletes — see [crates/klams-mcp/src/metrics.rs](../../crates/klams-mcp/src/metrics.rs)). Adds a `deploy/prometheus/` directory with the scrape job for `klams-service` so a clean checkout reproduces working dashboards (FR-018).
5. **Perf fixture + benchmark harness** — a new non-shipping `tools/bench/` Rust crate housing a deterministic, seeded fixture generator (≥ 10k facts + ≥ 50k knowledge items) and a 100-call `memory_search` latency harness that writes `sprints/008-activity-observability/perf-baseline.md` with p50/p95/p99. A `just bench-seed` + `just bench-run` recipe pair wires it up. The README gets a one-line link to the baseline.

Three cross-cutting design decisions baked in up front:

- **Two surfaces, one query layer** (R-001). The MCP tool and the HTTP endpoint deliberately do not collapse into a single surface — consumers, projections, and rate-limit profiles differ. They share a new `Store::list_memories(...)` trait method (companion to sprint 007's `list_author_memories`) that performs the date-windowed SQL + Qdrant query and returns rows already in the `PublicMemory` projection.
- **Window cap policy is global, not per-token** (R-002). Single 30-day knob in `Config::api.memories_max_window_days`, defaulting to 30. Out of scope: per-token windows, per-IP quotas, query-cost accounting.
- **Perf baseline surfaces measurement, never triggers tuning** (FR-022). The harness writes the markdown and exits. Any tuning is a future sprint subject to user review of the recorded numbers, regardless of where they land relative to the SC-006 1-second threshold.

## Technical Context

**Language/Version**: Rust 1.94.1 stable (`rust-toolchain.toml`), edition 2021 (workspace pinned in `Cargo.toml`). Viewport: SvelteKit + TypeScript on Tauri 2 (unchanged from sprint 007).
**Primary Dependencies**: existing — `tokio` 1.x, `axum` 0.7, `sqlx` 0.8 (Postgres), `qdrant-client` 1.12, `reqwest` 0.12, `tracing` 0.1, `prometheus` 0.13, `serde` 1, `serde_json` 1, `chrono` 0.4, `uuid` 1, `base64` 0.22, `rmcp` 1.7.0 (already wired). New (perf tooling only, non-shipping) — `rand` 0.8 + `rand_chacha` 0.3 for the seeded generator, `hdrhistogram` 7.x for the latency harness. No new runtime crate dependencies for the agent-facing or operator-facing deliverables.
**Storage**: Postgres 16 (existing `facts`, `events`, `authors` tables — no new tables, no new columns), Qdrant 1.12.4 (existing `knowledge_items` collection — payload-only reads), TEI HTTP embedder (unused on these read paths).
**Testing**: `cargo test --workspace` (unit + integration), `tests/docker-compose.test.yml` fixture, `just gate` (`cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`). New integration suites for `event_search`, `GET /v1/memories`, and the viewport `/activity` Tauri command. Manual quickstart validation for the Grafana panel fix (no headless Grafana in CI). Perf harness is a `cargo run -p klams-bench` invocation, not a `#[test]` — explicit operator-driven.
**Target Platform**: Linux server (kubs0 systemd unit) for `klams-service`. Viewport: cross-built for `x86_64-pc-windows-msvc` via `cargo-xwin` + native Linux for WSLg. Perf tooling: Linux only (cargo bin under `tools/bench`).
**Project Type**: Cargo workspace + Tauri/SvelteKit app + new non-shipping perf tooling crate. New crate `klams-bench` joins under `tools/bench/` (sibling to `crates/`); it is **not** a dependency of any production binary — it is added to the workspace `members` so `cargo check --workspace` keeps it green, but no shipping artifact links it.
**Performance Goals**: `GET /v1/memories` < 200 ms p95 for the default 24h window over the homelab corpus (≤ 10k facts, ≤ 50k knowledge items); `event_search` < 100 ms p95 over the same corpus; viewport `/activity` first-row render < 1 s from click (SC-002). Perf harness itself has no SLO — it measures `memory_search`.
**Constraints**: zero new unsafe code (workspace remains `unsafe_code = "forbid"`); no schema migrations (FR additive); `Memory` projection shape unchanged for live rows (soft-delete metadata only appears when `state ∈ {deleted, all}` — FR-010); Prometheus label cardinality preserved exactly as sprint 007 R-010 defines (`agent_name`, `model`, plus bounded `kind`/`mode` enums); 30-day window cap enforced **before** the query is dispatched (FR-009); cursor encoding compatible with sprint 007's `(created_at, id)` base64 format (no new cursor wire format).
**Scale/Scope**: single klams instance; default `/v1/memories` page size 50, max 200; default `event_search` page size 50, max 500 (matching `memory_admin_list_deleted`); benchmark fixture seeds ≥ 10k facts + ≥ 50k knowledge items in one run; ≤ 100 author rows over the system's lifetime (unchanged from sprint 007).

## Constitution Check

*Re-checked after Phase 1 design — both passes recorded below.*

| Principle | Initial gate (pre-design) | Post-Phase-1 gate | Notes |
|-----------|---------------------------|-------------------|-------|
| I. SDD | PASS | PASS | `spec.md` precedes any code; this plan + research/data-model/quickstart/contracts complete the SDD artifact set before `/speckit.tasks` is invoked. |
| II. TDD | PASS | PASS | Each FR maps to a contract or integration test slot listed under **Source Code** below. The JSON schema in `contracts/tool-schemas/event_search.json` and the REST contract in `contracts/rest-memories.md` drive contract tests that fail until the implementation lands. The perf harness has a deterministic-seed unit test (same seed → same fixture). |
| III. Code Standards | PASS | PASS | `just gate` is unchanged. No new lints relaxed. One new crate (`klams-bench`) added to the workspace lint set with the same `unsafe_code = "forbid"` and clippy baseline as the rest of the workspace. |
| IV. Documentation | PASS | PASS | `docs/usage.md` gains an "Activity tab" section + an `event_search` row in the tool table; `docs/architecture.md` §2f covers the shared query layer; `docs/setup.md` notes the new `deploy/prometheus/` scrape config; `README.md` adds the perf baseline link. All on the Phase 1 deliverables list below. |
| V. Quality & Observability | PASS | PASS | No new Prometheus counters are introduced — the sprint specifically **fixes** existing-counter visibility. Structured `tracing` spans extend to the new tool/endpoint with token-hash, `agent_name`, `model`, requested window, and result count (no PII; no `author_id` as a label). Error codes are stable contract surface: two additions (`WINDOW_TOO_LARGE`, `INVALID_WINDOW`) documented in [contracts/error-codes.md](./contracts/error-codes.md). |
| VI. Simplicity & Intentional Design | PASS | PASS | Reuse over invention: cursor encoding reused verbatim from sprint 007; projection types reused verbatim; `Store` trait extended (not replaced); MCP tool registration uses the existing `#[tool]` macro pipeline. Perf harness is one binary pair, one fixture, one markdown output — no test infrastructure beyond what `cargo run` provides. Two surfaces (MCP tool + REST endpoint) deliberately not collapsed (R-001) — collapsing would force one consumer to subset the other's projection or scope set. |

No principle violations require justification in **Complexity Tracking**.

## Project Structure

### Documentation (this feature)

```text
sprints/008-activity-observability/
├── spec.md                      # /speckit.specify output (zero NEEDS CLARIFICATION)
├── plan.md                      # this file
├── research.md                  # Phase 0 output (this run) — R-001..R-011
├── data-model.md                # Phase 1 output (this run) — query-layer types, no DB changes
├── quickstart.md                # Phase 1 output (this run) — 10-step walkthrough = acceptance script
├── contracts/                   # Phase 1 output (this run)
│   ├── README.md
│   ├── mcp-event-search.md      # event_search tool reference + scope + output shape
│   ├── rest-memories.md         # GET /v1/memories contract
│   ├── error-codes.md           # WINDOW_TOO_LARGE + INVALID_WINDOW additions
│   ├── grafana-mcp-panels.md    # Authoritative PromQL for the three MCP panels
│   ├── prometheus-scrape.md     # Authoritative scrape job for klams-service
│   ├── bench-harness.md         # CLI surface for the seed + run binaries + output format
│   └── tool-schemas/
│       └── event_search.json    # JSON Schema 2020-12 for event_search input
├── perf-baseline.md             # written by `just bench-run`; checked in
└── tasks.md                     # Phase 2 output (NOT created here — /speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── klams-store/
│   ├── src/lib.rs               # +ListMemoriesQuery, +ListMemoriesRow, +EventSearchQuery,
│   │                            #   +Store::list_memories, +Store::event_search trait methods
│   ├── src/composite.rs         # impls for the two new methods; shared date-window + cursor helper
│   ├── src/postgres.rs          # +list_memories_facts_page, +list_memories_events_page,
│   │                            #   +event_search_page (payload_match → JSONB containment)
│   └── src/qdrant.rs            # +list_memories_knowledge_page (payload-only date-window scroll)
├── klams-api/
│   ├── src/handlers/memories.rs # NEW: GET /v1/memories handler (read scope, window cap, projection)
│   ├── src/router.rs            # +mount /v1/memories under read scope
│   ├── src/error.rs             # +WindowTooLarge variant → 400 WINDOW_TOO_LARGE
│   │                            # +InvalidWindow variant → 400 INVALID_WINDOW (since > until)
│   └── (existing handlers unchanged)
├── klams-mcp/
│   ├── src/tools/event_search.rs # NEW: tool impl, schemars-derived input, exact-match payload filter
│   ├── src/tools/mod.rs          # +register event_search under read scope
│   └── src/errors.rs             # +WINDOW_TOO_LARGE, +INVALID_WINDOW error_code constants
├── klams-types/
│   └── src/config.rs            # +ApiConfig.memories_max_window_days: u32 (default 30)
└── (klams-service, klams-core, klams-client, klams-monitor, klams-scanner unchanged)

tools/                            # NEW directory (sibling to crates/)
└── bench/
    ├── Cargo.toml               # package `klams-bench`; workspace member; not depended on by any binary
    ├── src/bin/seed.rs          # deterministic seeded fixture generator
    ├── src/bin/run.rs           # 100-call memory_search harness, writes perf-baseline.md
    ├── src/lib.rs               # shared corpus generator + histogram → markdown serializer
    └── README.md                # operator-facing usage notes

viewport/
└── src/
    ├── routes/activity/
    │   ├── +page.svelte         # NEW: activity list view
    │   └── +page.ts             # NEW: SvelteKit loader → Tauri command
    ├── lib/types/memories.ts    # NEW: ListMemoriesRequest / MemoryItem types matching contract
    └── src-tauri/src/commands/
        └── memories.rs          # NEW: list_memories Tauri command → GET /v1/memories

deploy/
├── grafana/
│   └── klams.json               # +three "MCP author activity" panels (writes / deletes / search)
└── prometheus/                  # NEW directory
    ├── prometheus.yml           # scrape job for klams-service + commented compose-mode block
    └── README.md                # how it composes with the existing compose stack

tests/
├── integration/
│   ├── mcp_event_search.rs              # FR-001..FR-005 — filters, cursor, scope, no-embedding
│   ├── mcp_event_search_window.rs       # FR-002 + edge cases (empty, inverted, oversized)
│   ├── api_memories_list.rs             # FR-006..FR-011 — defaults, filters, projection, scope
│   ├── api_memories_window_cap.rs       # FR-009 — WINDOW_TOO_LARGE error body shape
│   ├── api_memories_deleted_state.rs    # FR-010 — deleted_at + deleted_by_author_id surfacing
│   ├── viewport_activity_command.rs     # FR-014 — Tauri command round-trip (mocks klams-service)
│   └── store_list_memories.rs           # cross-author, multi-kind, cursor continuity
├── unit/
│   ├── klams-store/cursor_v2.rs         # cursor encode/decode parity with sprint 007
│   └── klams-bench/fixture_determinism.rs   # same seed → same corpus
└── fixtures/
    └── memories/                        # seed data covering all kinds + soft-deleted rows

docs/
├── architecture.md              # +§2f Activity & Observability — shared query layer, panel fix
├── setup.md                     # +deploy/prometheus/ wiring + Grafana reload note
└── usage.md                     # +Activity tab section, +event_search row in MCP tool table

README.md
└── +link: "[Performance baseline](sprints/008-activity-observability/perf-baseline.md)"

justfile
├── +bench-seed                  # `cargo run -p klams-bench --bin seed`
└── +bench-run                   # `cargo run -p klams-bench --bin run`
```

**Structure Decision**: extend existing crates (`klams-store`, `klams-api`, `klams-mcp`) for the agent-facing and operator-facing surfaces, and add **one** new non-shipping workspace crate (`tools/bench/`) for the perf fixture and harness. Justification:

- The MCP `event_search` tool and the REST `GET /v1/memories` endpoint share a SQL query layer (`Store::list_memories`) so they belong in `klams-store`; their thin frontends belong in `klams-mcp` and `klams-api` respectively. No new shipping crate is warranted — the surface is small and uses identical primitives to sprint 007's author drilldown.
- The perf fixture + harness must not be linked into any production binary (it pulls `rand`, `rand_chacha`, and `hdrhistogram` only for measurement), but it must be reachable from `cargo`-style developer workflows. A workspace member crate under `tools/bench/` keeps it under `just gate` (so it stays compilable and lint-clean alongside the rest of the workspace) while keeping `cargo build -p klams-service --release` unaffected.
- `deploy/prometheus/` is a new directory because the Prometheus scrape config is currently absent from the repo — sprint 007's metrics were added but the scrape side was assumed to live on the operator's host. Checking it in is the cheapest fix that satisfies FR-018 ("reproducible from a clean checkout") and gives the Grafana panels something to point at.

## Complexity Tracking

> No Constitution Check violations require justification.

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|--------------------------------------|
| _(none)_  | _(none)_   | _(none)_                             |
