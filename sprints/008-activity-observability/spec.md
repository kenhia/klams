# Feature Specification: Activity & Observability

**Feature Branch**: `008-activity-observability`
**Created**: 2026-05-25
**Status**: Draft
**Input**: Sprint 008 brief consolidating four PRIORITY backlog items (`event_search` MCP tool, viewport activity-by-date-range tab, Grafana "MCP author activity" panel fix, SC-006 perf benchmark) plus a complementary cross-author HTTP listing endpoint. References: [sprints/007-mcp-server/spec.md](../007-mcp-server/spec.md), [sprints/planning/backlog.md](../planning/backlog.md), [sprints/planning/plan.md](../planning/plan.md) Phase 7.

## Overview

Sprint 007 shipped the MCP server, authors table, and per-author Prometheus labels. Three usability gaps surfaced during that ship:

1. **Events are invisible to `memory_search`** — events have no embeddable text body, only `category` + structured `payload`, so the only way to retrieve them today is by reading them straight out of the `events` table.
2. **The viewport has no way to ask "what's new?"** — the only listing surfaces are scoped to a single author (`/authors/{id}`) or to a fact/knowledge/event kind in isolation.
3. **The Grafana "MCP author activity" panel renders "No Data"** even though `klams_mcp_*` counters scrape correctly from `/metrics`, so SC-005 from sprint 007 is unverified.

This sprint closes those gaps and establishes a repeatable performance baseline so SC-006 from sprint 007 stops being aspirational. The work is intentionally additive — no schema migrations, no new dependencies on external services, and no changes to the public `PublicMemory` projection (the `klams_types::PublicMemory` type exported by sprint 007).

The two new listing surfaces are deliberately **separate**: an agent-facing MCP tool (`event_search`) returns only events and is tuned for agents that already know how to talk MCP; an operator-facing HTTP endpoint (`GET /v1/memories`) returns the unified `PublicMemory` projection across all kinds and backs the viewport. They share an underlying date-windowed SQL query layer but expose two distinct surfaces with distinct rate-limit profiles.

## Clarifications

### Session 2026-05-25

- (none — all `[NEEDS CLARIFICATION]` markers resolved)

## User Scenarios & Testing *(mandatory)*

> **Convention**: each user story below has an "**Independent Test**" (one-paragraph operator-narrative summary of how to demo the story in isolation) followed by "**Acceptance Scenarios**" (BDD list — the source of truth for acceptance). The Independent Test is the prose form; the Acceptance Scenarios are the testable form.

### User Story 1 — Agent finds a recent event by filter (Priority: P1)

A controller-driven agent wants to know "when did widget last deploy to kub3?" or "show me every `MaintenanceWindow` event in the last hour." `memory_search` cannot serve this — events have no embeddable text — so the agent calls the new `event_search` MCP tool with `category` + `since` filters and gets the rows back directly.

**Why this priority**: Events were added to klams precisely to capture deployment, maintenance, and task-completion breadcrumbs. Without a retrieval path they are write-only, which defeats the purpose. This is the largest correctness gap left over from sprint 007.

**Independent Test**: Append several events with distinct `category` and `payload` values via `memory_append_event`. From a separate MCP client, call `event_search({category: "Deploy", since: "now − 1 h"})` and confirm only matching events are returned, newest first, with author attribution attached.

**Acceptance Scenarios**:

1. **Given** events of multiple categories exist within the last hour, **When** an agent calls `event_search({category: "Deploy", since: "<1h ago>"})`, **Then** only `Deploy` events created within the window are returned, ordered newest-first by default, each carrying `author.agent_name` and `author.model`.
2. **Given** events with structured payloads, **When** an agent calls `event_search({payload_match: {"service": "widget"}})`, **Then** only events whose payload contains `service == "widget"` (exact-equality match) are returned.
3. **Given** more than the page limit's worth of matching events, **When** an agent calls `event_search` with `limit: 50`, **Then** at most 50 events are returned and the response includes a cursor (or equivalent continuation token) for the next page.
4. **Given** a `read`-only token, **When** the agent calls `event_search`, **Then** the call succeeds (the tool is read-classified).
5. **Given** an agent passes both `since` and `until`, **When** the window is empty, **Then** the response is an empty list with no error.

---

### User Story 2 — Operator browses recent memory activity across all authors (Priority: P1)

Ken (or any operator running the viewport) opens the new "Activity" tab and sees every memory item — facts, knowledge, events — written in the last 24 hours, across every author, with one row per item and the familiar kind/state/tags/deep-link rendering. He can narrow by date range, kind, state (live / soft-deleted / all), and optionally by author.

**Why this priority**: This is the "what just happened?" view that an operator currently has to assemble by visiting `/facts`, `/knowledge`, `/events` individually and mentally merging the timelines. It is also the natural review surface for the soft-delete safety story shipped in sprint 007 — "show me everything that was deleted today" should be one click.

**Independent Test**: Seed klams with a handful of writes from two distinct authors across all three kinds, in the last day. Open the viewport, click "Activity", and confirm every seeded row appears with correct kind badge, state, tags, and author attribution. Toggle the kind filter to "event" and confirm only events remain. Set the state filter to "soft-deleted" and confirm only previously-deleted rows appear.

**Acceptance Scenarios**:

1. **Given** writes from multiple authors across all kinds in the last 24h, **When** Ken opens `/activity` in the viewport, **Then** he sees a single unified list, newest-first, defaulting to the last 24h window with the live-only state filter active.
2. **Given** the Activity view is open, **When** Ken changes the kind filter to "event", **Then** only event rows remain; **When** he changes it back to "all", **Then** all kinds reappear.
3. **Given** rows that have been soft-deleted, **When** Ken sets the state filter to "soft-deleted", **Then** only those rows appear and each row visibly indicates its deleted state.
4. **Given** more than the page limit's worth of items, **When** Ken scrolls / clicks "next page", **Then** the next batch loads via cursor pagination without resetting the filters.
5. **Given** Ken clicks an item row, **When** the navigation completes, **Then** he lands on the existing per-kind detail view for that item (facts → `/facts/:id`, knowledge → `/knowledge/:id`, events → `/events/:id`).

---

### User Story 3 — HTTP client lists cross-author memory activity (Priority: P1)

Any client with a `read`-scoped token can issue `GET /v1/memories?since=…&until=…&kinds=…&state=…&authors=…&limit=…&cursor=…` and receive a paginated cross-author, all-kinds memory listing in the same `Memory` projection shape returned everywhere else. The viewport's Activity tab is the first consumer; future controllers, dashboards, or scripts can reuse the same endpoint.

**Why this priority**: This is the API contract that the viewport Activity tab depends on, and it is a generally useful operator surface in its own right. Splitting it from US2 makes the REST contract independently testable from the desktop UI work.

**Independent Test**: With fixture data, call `GET /v1/memories?since=<24h ago>` and confirm the response is a JSON list of `Memory` items in the public projection (no internal fields), ordered newest-first, with a cursor in the response when more pages exist.

**Acceptance Scenarios**:

1. **Given** the default 24h window, **When** a client issues `GET /v1/memories` with no query parameters, **Then** the response contains every live memory item written in the last 24 hours across all kinds and authors.
2. **Given** a window narrower than the default, **When** a client passes both `since` and `until`, **Then** the response is restricted to that window (inclusive `since`, exclusive `until`).
3. **Given** `kinds=fact,event`, **When** the endpoint runs, **Then** knowledge items are omitted from the response.
4. **Given** `state=all`, **When** the endpoint runs, **Then** soft-deleted rows are included and each carries its `deleted_at` and `deleted_by_author_id` in the projection.
5. **Given** `authors=<uuid1>,<uuid2>`, **When** the endpoint runs, **Then** only items authored by one of those registrations are returned.
6. **Given** a token without the `read` scope, **When** the endpoint is called, **Then** the response is `403 INSUFFICIENT_SCOPE` and no data is returned.
7. **Given** a requested window larger than the configured maximum (see FR-009), **When** the endpoint is called, **Then** the response is `400 WINDOW_TOO_LARGE` with the configured maximum surfaced in the error body.

---

### User Story 4 — Operator sees real data in the Grafana "MCP author activity" panel (Priority: P2)

After sprint 007 the `klams_mcp_*` counters emit correctly and scrape with `curl /metrics`, but the corresponding Grafana panels render "No Data." Ken restarts the affected services after the fix lands and sees real time-series for writes, deletes, and searches broken down by `agent_name` and `model`.

**Why this priority**: Closes sprint 007's SC-005 without code changes to the service. Pure ops/config fix, but it is the only way to verify the per-author observability investment paid off.

**Independent Test**: Drive MCP traffic with at least one write, one delete, and one search from a registered author. Confirm the counter values exist in `/metrics`. Restart Prometheus and Grafana per the deployment runbook. Confirm the relevant panels in the Grafana dashboard render non-empty time series for the corresponding labels.

**Acceptance Scenarios**:

1. **Given** the fix has been applied and Prometheus + Grafana have been restarted, **When** Ken opens the klams Grafana dashboard, **Then** the "MCP author activity" panel renders real series for `klams_mcp_writes_total`, `klams_mcp_deletes_total`, and `klams_mcp_search_total`, broken down by `agent_name` and `model`.
2. **Given** the panels render, **When** new MCP traffic is generated, **Then** the panel updates within Prometheus' configured scrape interval without further intervention.
3. **Given** the fix is checked in, **When** another operator pulls the repo and runs the deploy steps from scratch, **Then** the panels render without manual Prometheus / Grafana config tweaks beyond what is documented.

---

### User Story 5 — Operator measures a reproducible `memory_search` performance baseline (Priority: P2)

A deterministic fixture generator seeds a fresh test database with the homelab's nominal store size (≥ 10k facts + ≥ 50k knowledge items), then a benchmark harness runs `memory_search` 100× and records p50/p95/p99 latencies. The result is checked into the repo as a markdown artifact and linked from the top-level `README.md`. The harness can be re-run on demand by any future sprint that touches the search path.

**Why this priority**: Sprint 007 declared SC-006 (`memory_search` p95 < 1s at homelab scale) but never measured it. Establishing the baseline once unblocks every future "did my change regress search?" question.

**Independent Test**: Run `just bench-seed` (or the equivalent recipe) against a fresh test DB and Qdrant collection; confirm the row counts. Run the benchmark harness 100× against a representative query; confirm the output markdown file is generated under `sprints/008-activity-observability/` (or the agreed path), contains p50/p95/p99 numbers, and is referenced from `README.md`.

**Acceptance Scenarios**:

1. **Given** the fixture generator is invoked with a fixed seed, **When** it runs against a clean store, **Then** at least 10k facts and at least 50k knowledge items are produced, and a second run with the same seed produces an equivalent corpus (deterministic).
2. **Given** the seeded store, **When** the benchmark harness runs `memory_search` 100 times with a representative query, **Then** p50, p95, and p99 latencies are recorded and persisted to a markdown file inside the sprint's spec directory.
3. **Given** the perf baseline markdown exists, **When** a reader opens the top-level `README.md`, **Then** there is a visible link to the baseline file.
4. **Given** a p95 measurement above the SC-006 1-second threshold, **When** the harness reports it, **Then** no tuning work is started automatically — the result is surfaced and the next step is a user decision.

---

### Edge Cases

- **Empty store**: `event_search` and `GET /v1/memories` against an empty (or all-pre-window) store return an empty list, never an error.
- **Future `until`**: `since` or `until` set to a future timestamp is accepted; the empty intersection just returns an empty list.
- **Inverted window**: `since > until` returns `400 INVALID_WINDOW`.
- **Window exceeds configured maximum**: see US3 acceptance #7. Same rule applies to `event_search` (see FR-002).
- **Pagination during writes**: rows added between page fetches may or may not appear on later pages; cursor pagination is keyed on `(created_at, id)` so an item added strictly after the first page's cursor will appear on a later page, but live updates while the operator scrolls are not promised.
- **Author filter with unknown UUID**: an unknown `authors` UUID is silently ignored (no error); a list containing only unknown UUIDs yields an empty result.
- **`state=all` with `kinds=event`**: events are append-only and never carry deletion state, so the `state` filter is a no-op when the only requested kind is `event`. The endpoint MUST NOT error.
- **Grafana fix has no behavioral test in CI**: the fix is verified by a manual quickstart step against a running compose stack, not by an automated test (no Grafana available in CI today).
- **Benchmark on a host smaller than `kubs0`**: the harness reports the measured numbers without comparison to SC-006's 1-second threshold; the threshold applies only on the homelab's reference host.
- **Concurrent activity during benchmark**: the bench harness assumes a quiescent store; concurrent writes during the run will skew the measurement and the operator is expected to run it against a freshly-seeded test environment, not production.

## Requirements *(mandatory)*

### Functional Requirements

**`event_search` MCP tool**

- **FR-001**: The MCP server MUST provide an `event_search` tool that returns events filtered by any combination of `author_id`, `category` (string or list), `since`, `until`, and `payload_match` (an object of key → value pairs requiring exact equality on the event's `payload`).
- **FR-002**: `event_search` MUST accept a `limit` parameter (default 50, maximum 500) and an `order` parameter (`"desc"` default, `"asc"` allowed). It MUST support cursor pagination on `(created_at, id)`.
- **FR-003**: `event_search` MUST require the `read` scope and be visible in `tools/list` for any token holding `read`.
- **FR-004**: `event_search` MUST NOT invoke the embedding pipeline; it is a pure SQL query.
- **FR-005**: Results MUST be returned in the existing public `PublicMemory` projection with `kind: "event"`, including the same `author` subset used elsewhere (`agent_name`, `model`, `repo`).

**`GET /v1/memories` HTTP endpoint**

- **FR-006**: `klams-service` MUST expose `GET /v1/memories` returning a paginated list of memory items in the public `PublicMemory` projection, across all configured kinds and all authors, gated by the `read` scope.
- **FR-007**: The endpoint MUST accept the following query parameters: `since` (RFC3339; default `now − 24h`), `until` (RFC3339; default `now`), `kinds` (comma-separated subset of `fact,knowledge,event`; default all), `state` (`live` | `deleted` | `all`; default `live`), `authors` (comma-separated UUIDs; default unrestricted), `limit` (default 50, maximum 200), and `cursor` (opaque).
- **FR-008**: The endpoint MUST default-sort newest-first by `(created_at, id)` and return a cursor when more results are available, using the same pattern as `GET /v1/authors/{id}/memories` from sprint 007.
- **FR-009**: The endpoint MUST enforce a maximum allowed window between `since` and `until`, configured via `Config::api.memories_max_window_days` (default **30 days**). The same cap applies to `event_search` (FR-002). A request exceeding the configured maximum MUST return `400 WINDOW_TOO_LARGE` with the configured value surfaced in the error body. The 30-day default covers the operator "what's happened recently" use case while bounding the worst-case scan against the growing store; retrospective queries beyond the cap must paginate by window.
- **FR-010**: The endpoint MUST reuse the same `PublicMemory` projection shape returned by the MCP `memory_search` tool — no new fields, no internal-only fields leaked. Soft-deleted rows returned when `state ∈ {deleted, all}` MUST additionally carry their `deleted_at` and `deleted_by_author_id` in the projection.
- **FR-011**: The endpoint MUST NOT invoke the embedding pipeline; it is a pure SQL / Qdrant filter query, sharing the underlying date-windowed query layer with `event_search`.

**Viewport Activity tab**

- **FR-012**: The viewport MUST gain a top-level `/activity` route with a nav-bar entry labelled "Activity". The route MUST render the same row-rendering primitives as the per-author drilldown (kind badge, state, summary, tags, deep-link per memory).
- **FR-013**: The Activity view MUST expose UI controls for: from/to datetime pickers (default last 24h), kind filter (fact / knowledge / event / all), state filter (live / soft-deleted / all), author multi-select (optional, default unrestricted), and a "next page" affordance driven by the response cursor.
- **FR-014**: The Activity view MUST be backed by a new Tauri command and TypeScript type that wrap `GET /v1/memories` exactly; no new server contract is added beyond US3.
- **FR-015**: Clicking a row MUST navigate to the existing per-kind detail route (`/facts/:id`, `/knowledge/:id`, `/events/:id`).
- **FR-015a**: Clicking a soft-deleted row in the Activity view MUST navigate to the per-kind detail route and that detail view MUST render the soft-deleted state (`state`, `deleted_at`, `deleted_by`) without error. (Verified by T054.)

**Grafana panel fix**

- **FR-016**: The deployed Prometheus scrape configuration MUST successfully scrape every `klams_mcp_*` metric emitted by `klams-service`, with the same label set documented in sprint 007 (`agent_name`, `model`, `kind`, `mode`).
- **FR-017**: The Grafana dashboard JSON checked into `deploy/grafana/` MUST contain panel queries (PromQL) that match the labels actually emitted by sprint 007's `klams_mcp_*` counters, so that the "MCP author activity" panels render real data after a `docker-compose restart prometheus grafana`.
- **FR-018**: The fix MUST be reproducible from a clean checkout: a fresh `git clone` + the deployment runbook in `docs/setup.md` (or `deploy/`) MUST yield working panels without ad-hoc manual edits.

**Performance baseline**

- **FR-019**: The repository MUST contain a deterministic, seeded fixture generator (e.g. `tools/bench/seed.rs` invoked via a `just bench-seed` recipe) that loads at least 10,000 facts and at least 50,000 knowledge items into a fresh test database and Qdrant collection. Running the generator twice with the same seed MUST produce equivalent corpora.
- **FR-020**: The repository MUST contain a benchmark harness that runs `memory_search` 100 times against the seeded store and records p50/p95/p99 latencies. The harness MUST be re-runnable on demand.
- **FR-021**: The benchmark results MUST be persisted as a markdown artifact inside the sprint's spec directory (`sprints/008-activity-observability/perf-baseline.md`) and MUST be linked from the top-level `README.md`.
- **FR-022**: The benchmark harness MUST NOT auto-trigger tuning work if a measurement exceeds the SC-006 threshold. Surfacing the measurement is the entire deliverable; any tuning is a follow-up sprint subject to user review.

### Key Entities

- **PublicMemory (public projection)**: the `klams_types::PublicMemory` type exported by sprint 007 — unchanged here. Reused verbatim by `event_search`, `GET /v1/memories`, and the viewport Activity tab. The only addition is the optional surfacing of `deleted_at` + `deleted_by_author_id` when the projection represents a soft-deleted row (already a property of the underlying row; sprint 007 hid these from default search but the cross-author listing intentionally exposes them when `state ∈ {deleted, all}`).
- **Date-windowed query layer**: a shared SQL helper (likely in `klams-store`) that takes `(since, until, kinds, state, authors, limit, cursor)` and returns rows in the public projection. Backs both the new MCP tool (for events) and the new HTTP endpoint (for all kinds).
- **Cursor**: opaque continuation token, base64-encoded over `(created_at, id)`. Same shape as the cursor used by `GET /v1/authors/{id}/memories` in sprint 007 — no new wire-format invention.
- **Benchmark fixture corpus**: a deterministic, seeded synthetic corpus of facts and knowledge items. Not part of the production data model; lives entirely in test infrastructure.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An agent holding a `read`-scoped token can retrieve every event in a one-hour window by category in a single `event_search` call, without resorting to direct SQL.
- **SC-002**: Opening the viewport "Activity" tab against a non-empty klams shows the last 24 hours of memory activity in under 1 second from click to first row rendered, with all kinds and all authors merged into one chronological list.
- **SC-003**: The Grafana "MCP author activity" panel renders non-empty time series for `klams_mcp_writes_total`, `klams_mcp_deletes_total`, and `klams_mcp_search_total` (broken down by `agent_name` and `model`) within one Prometheus scrape interval after the next MCP call following the fix.
- **SC-004**: The seeded perf fixture produces at least 10,000 facts and at least 50,000 knowledge items in a single run, and the benchmark harness produces a markdown report with p50, p95, and p99 latency numbers for `memory_search` over a 100-call sample.
- **SC-005**: A new contributor following the top-level `README.md` can locate and read the perf baseline within 30 seconds of opening the README, without prior knowledge of the sprint number.
- **SC-006**: Every existing sprint 007 acceptance test continues to pass without modification — the new tool, endpoint, and viewport route are additive and do not alter pre-existing contracts.

## Assumptions

- **Underlying query layer is shared, but the surfaces are separate.** The agent-facing `event_search` MCP tool and the operator-facing `GET /v1/memories` HTTP endpoint sit on the same SQL helper but expose two distinct surfaces. They are not collapsed into a single tool/endpoint because the consumers, the projections (events-only vs all-kinds), and the rate-limit profiles differ.
- **No schema migrations.** All queries operate over columns and indexes that already exist after sprint 007 (`authors`, `facts`, `events`, the knowledge payload in Qdrant, soft-delete columns on facts).
- **No new auth scopes.** The `read` scope from sprint 007 is sufficient for both `event_search` and `GET /v1/memories`. The Grafana panel fix and the perf benchmark touch deployment + tooling, not the auth layer.
- **Grafana fix is a config drift, not a service change.** The investigation starts with `deploy/prometheus/` scrape targets and `deploy/grafana/` panel PromQL. If the root cause turns out to be a missing service-side label or metric, the plan phase will widen scope and flag it; the spec assumes the fix is reachable without touching `klams-service` code.
- **Perf fixture is a Rust binary under `tools/`.** A deterministic, seeded generator wins over a pre-generated artifact because the schema is still evolving and a captured artifact would drift. `just bench-seed` (or equivalent) is the operator-facing entry point.
- **Perf results live in the sprint's spec directory.** Filing the baseline under `sprints/008-activity-observability/perf-baseline.md` keeps the measurement bound to the sprint that created it; the `README.md` link is the discoverability surface.
- **Perf re-run policy.** Any future sprint that includes changes likely to affect search performance is expected to re-run the harness and append (or replace) the baseline. The harness itself does not need to enforce this — it is a documentation and review-process commitment.
- **Cursor pagination semantics.** `(created_at, id)` is stable enough for the operator's "what just happened?" workflow even under concurrent writes. Strong snapshot semantics are out of scope.
- **Window-max policy.** Out of scope items: per-token window limits, per-IP rate limits, query-cost accounting. A single global maximum (FR-009) is the only knob.
- **Reproducibility baseline.** The 10k facts + 50k knowledge corpus size is chosen to mirror the test-environment scale used to size the sprint-007 store; future runs against `kubs0` must meet or exceed this corpus before their numbers are comparable. Representative queries (see [contracts/bench-harness.md § Query Set Governance](./contracts/bench-harness.md#query-set-governance)) span the three kinds and a mix of single-term, multi-term, and combined queries; the query set is checked in and evolves only when a new use case warrants it.

- **Viewport detail navigation.** The existing per-kind detail routes (`/facts/:id`, `/knowledge/:id`, `/events/:id`) already exist and render the soft-deleted state appropriately. No rework of those routes is in scope; FR-015a + T054 verify this assumption rather than re-build it.

## Dependencies

- Sprint 007 (MCP server, authors table, soft-delete columns, per-author Prometheus labels, `GET /v1/authors/{id}/memories`) is shipped. This sprint reuses every primitive sprint 007 introduced.
- The existing `klams-store`, `klams-api`, `klams-service`, `klams-mcp`, and `viewport/` crates/packages gain additive changes only — no new top-level crates.
- A small new tooling crate (or set of harness scripts) under `tools/bench/` is introduced for the perf fixture and harness. It is non-shipping infrastructure, not a runtime dependency of any klams binary.

## Out of Scope

- Tuning work in response to the perf baseline — gated by user review of the measurement.
- Viewport `source` / trust-rank rendering and dedupe/decay weight surfacing (separate "viewport UX" sprint).
- Usefulness-signal decay boost (separate sprint).
- Phase 6 test harness isolation for `tests/mcp_phase6.rs` (low-priority backlog, deferred).
- Per-token rate limiting or per-IP quotas for the new endpoint.
- Any change to the existing `Memory` projection shape beyond optionally surfacing `deleted_at` / `deleted_by_author_id` when `state ∈ {deleted, all}`.
