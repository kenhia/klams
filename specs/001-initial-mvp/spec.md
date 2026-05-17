# Feature Specification: klams Initial MVP (service + viewport)

**Feature Branch**: `001-initial-mvp`
**Created**: 2026-05-16
**Status**: Draft
**Input**: Build the initial MVP of klams — Ken's Local Agent Memory System —
combining Phase 0 foundations and Phase 1 memory service from
[plan.md](../planning/plan.md), and Phase 0 scaffold + Phase 1 memory
inspector for the desktop viewport from [viewport.md](../planning/viewport.md).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Write and retrieve a fact end-to-end (Priority: P1)

Ken (or his controller) records a stable fact about his environment
(e.g., "kubs0 has 64 GB RAM") into klams and later retrieves it via a
unified search call. This is the smallest end-to-end slice that proves
the API, queue, worker pipeline, and Postgres-backed `facts` store all
work together.

**Why this priority**: Facts are the foundation of user and task memory.
Without write + read of a single fact type, nothing else in klams has
value. This is the minimum viable slice.

**Independent Test**: From a controller-side `curl` (or the `klams-client`
crate), `POST` a user fact to `/memory/facts`, then `POST` a search query
to `/memory/search` filtered to user facts and confirm the new fact is
returned with its full payload, source, and timestamps.

**Acceptance Scenarios**:

1. **Given** a running klams service with an empty `facts` table,
   **When** the controller posts a well-formed user fact to
   `/memory/facts`, **Then** the service responds with the persisted
   fact id within 1 second and the fact is visible via
   `GET /memory/facts` and via `/memory/search`.
2. **Given** a fact already exists with the same `(type, canonical payload)`,
   **When** the controller posts an upsert for that fact, **Then** the
   service updates the existing row (incrementing `version` and
   `updated_at`) rather than creating a duplicate.
3. **Given** the service is restarted, **When** the controller queries the
   fact by id, **Then** the fact is still present with the same content
   and `created_at`.

---

### User Story 2 - Append and query task events (Priority: P1)

The controller appends events to an append-only event log
(e.g., "ansible play X completed on kubs0"), then queries recent events
filtered by `task_id` and `category` to reconstruct what happened during a
task.

**Why this priority**: Events are how task memory accumulates over time.
The controller and future agents both need a durable, ordered record of
what happened, and the viewport relies on it for the Events view.

**Independent Test**: Post N events with mixed `task_id` and `category`
values to `/memory/events`, then query by `task_id` and confirm only
matching events return, in chronological order.

**Acceptance Scenarios**:

1. **Given** an empty `events` table, **When** the controller appends
   three events with different `(task_id, category)` combinations,
   **Then** all three are persisted and a filtered query for one
   `task_id` returns only the matching events, ordered by `created_at`
   ascending.
2. **Given** an event has already been appended, **When** any client
   attempts to update or delete it, **Then** the request is rejected
   (append-only).

---

### User Story 3 - Index a knowledge chunk and retrieve it semantically (Priority: P1)

Ken (or a controller-side scanner stub) submits a text chunk with metadata
(`source`, `tags`, `repo`, `file`) to `/memory/knowledge/index`. The
service generates an embedding on the kubs0 GPU and stores it in Qdrant.
A subsequent `/memory/search` call returns the chunk for a semantically
similar query.

**Why this priority**: Knowledge memory is what makes klams more than a
fact database. Vector retrieval driven by GPU-backed embeddings is the
defining MVP capability.

**Independent Test**: Index three text chunks on distinct topics, issue a
search query whose meaning matches one of them, and verify that chunk is
ranked first with a non-trivial score.

**Acceptance Scenarios**:

1. **Given** an empty `knowledge_items` collection, **When** the controller
   submits a 200–800 token text chunk, **Then** the service accepts the
   request, returns a `knowledge_id` immediately (write is enqueued), and
   within 10 seconds the chunk is searchable.
2. **Given** three indexed chunks on unrelated topics, **When** a
   semantically related query is submitted to `/memory/search`,
   **Then** the most relevant chunk is the top result.
3. **Given** the same chunk is submitted twice with identical content,
   **When** the second request is processed, **Then** the service
   deduplicates via content hash and does not create a second vector.
4. **Given** the service is restarted, **When** the search query is
   re-run, **Then** the same top result is returned (embeddings persisted).

---

### User Story 4 - Unified search across memory types (Priority: P2)

A client (controller, viewport, or future agent) issues a single
`/memory/search` call with a query, optional `types` filter, optional
metadata filters, and a `top_k`. The service returns a merged, ranked
result set that may include facts, events, and knowledge items.

**Why this priority**: Unified search is what differentiates klams from
"a Postgres table plus a Qdrant collection". It is also the primary API
the viewport's search bar uses.

**Independent Test**: Seed one fact, one event, and one knowledge chunk
sharing a keyword. Query `/memory/search` for that keyword with
`types: ["fact","event","knowledge"]` and confirm all three appear in
the response with type-tagged entries.

**Acceptance Scenarios**:

1. **Given** seeded entries of each memory type, **When** a client posts
   a search with `top_k=10` and no `types` filter, **Then** the response
   contains entries of multiple types, each tagged with its `type` and a
   score.
2. **Given** the same seed, **When** the client restricts `types` to
   `["knowledge"]`, **Then** only knowledge items are returned.

---

### User Story 5 - Operate the service on kubs0 with observability (Priority: P2)

The service runs as a long-lived process on `kubs0` (managed by systemd
in the MVP environment), exposes `/healthz` and `/metrics`, and emits
structured logs. Ken can confirm it is healthy and inspect queue depth,
worker counts, and write throughput from Prometheus.

**Why this priority**: Without health and metrics, debugging memory
issues from a headless Linux host is impractical. Constitution
principle V (Quality & Observability) requires this.

**Independent Test**: Hit `/healthz` and `/metrics` from another host on
the LAN; confirm `/healthz` returns 200 with subsystem status (Postgres,
Qdrant, embedding worker), and `/metrics` exposes the named gauges and
counters listed in FR-018.

**Acceptance Scenarios**:

1. **Given** the service is running with Postgres and Qdrant up,
   **When** a client calls `/healthz`, **Then** it returns 200 and a
   JSON body listing each subsystem as `ok`.
2. **Given** Qdrant is intentionally stopped, **When** `/healthz` is
   called, **Then** it returns a non-200 status with the failing
   subsystem named in the body.
3. **Given** write traffic is flowing, **When** `/metrics` is scraped,
   **Then** queue depth, worker count, write throughput, and write
   latency histograms are present and non-zero.

---

### User Story 6 - Inspect memory from the Windows viewport (Priority: P1)

Ken launches `klams-viewport.exe` on his Windows workstation. It connects
to the klams service on `kubs0`, shows connection status, and lets him
browse facts, events, and knowledge items in three filterable views with
a detail pane.

**Why this priority**: The Linux hosts are headless. Without the
viewport, the only way to inspect klams state is `curl` and `psql`,
which is exactly the friction the viewport is being built (early) to
remove. Viewport completes the MVP by making the system human-debuggable.

**Independent Test**: With the service populated by the earlier stories,
launch the viewport on Windows, confirm it shows a green connection
indicator, navigate to each of the three memory views, apply at least
one filter per view, and open the detail pane on one entry.

**Acceptance Scenarios**:

1. **Given** the viewport binary is installed on a Windows workstation
   and configured with the kubs0 URL and bearer token,
   **When** Ken launches it, **Then** the dashboard shows the service
   URL, app version, and a green health indicator within 3 seconds.
2. **Given** facts, events, and knowledge items exist, **When** Ken
   opens the Facts view and filters by `type=UserFact`, **Then** only
   matching facts are shown with columns for payload preview,
   `confidence`, `last_used_at`, and `use_count`.
3. **Given** an entry is selected, **When** Ken opens the detail pane,
   **Then** the full payload is shown along with `id`, `source`, and
   timestamps, and a "copy id" action is available.
4. **Given** the service is unreachable, **When** the viewport refreshes,
   **Then** the connection indicator turns red and a non-blocking error
   message describes the failure.

---

### Edge Cases

- A client posts a fact whose `payload` does not match the structure
  expected for its `type`. The service rejects with a 400 and an
  actionable error message; nothing is persisted.
- A client submits a knowledge chunk longer than the configured maximum
  (default 8 KB). The service rejects with 413 and the configured limit
  is reported in the error body.
- The write queue is full (bounded `mpsc`). New write requests receive
  503 with a retry-after hint; existing in-flight work continues.
- The embedding worker fails for a single chunk (e.g., GPU OOM). The job
  is retried up to `N` times (default 3) with backoff; after exhaustion
  the chunk is moved to a dead-letter table and a metric is incremented.
- A search query returns zero results. The response is a successful 200
  with an empty `results` array, not an error.
- Postgres is reachable but Qdrant is down at startup. The service starts
  in degraded mode: fact and event APIs work, knowledge endpoints return
  503, `/healthz` reflects the degraded subsystem.
- The viewport's bearer token is wrong. The viewport surfaces a 401 with
  guidance to update the token in the config file, and does not retry
  silently in a tight loop.
- The viewport is launched while offline. It shows the cached
  configuration, a red connection indicator, and an explicit "retry
  connection" action; it does not crash.

## Requirements *(mandatory)*

### Functional Requirements

**Service: API and pipeline**

- **FR-001**: The service MUST expose `POST /memory/facts` to create or
  upsert a fact and return the persisted `id` and `version`.
- **FR-002**: The service MUST expose `GET /memory/facts` supporting
  filters on `type`, `source`, and a created-at time range, with
  pagination.
- **FR-003**: The service MUST expose `POST /memory/events` to append a
  single event; the resulting row MUST be immutable.
- **FR-004**: The service MUST expose `GET /memory/events` supporting
  filters on `task_id`, `category`, and a created-at time range, with
  results ordered by `created_at` ascending.
- **FR-005**: The service MUST expose `POST /memory/knowledge/index` that
  accepts text plus metadata, enqueues an embed-and-index job, and
  returns a `knowledge_id` synchronously.
- **FR-006**: The service MUST expose `POST /memory/search` accepting a
  `query` string, optional `types` array (subset of
  `["fact","event","knowledge"]`), optional metadata filters, and
  `top_k` (default 10, max 100); it MUST return a merged ranked result
  set with each entry tagged by `type` and a numeric `score`.
- **FR-007**: All write endpoints MUST enqueue work onto a bounded
  in-process queue and return after the job is accepted, not after the
  underlying store write completes (except `facts` upserts, which return
  after persistence so the caller has the canonical `id` and `version`).
- **FR-008**: A homogeneous worker pool MUST drain the queue, performing
  validation, dedupe, and store writes; pool size and queue capacity
  MUST be configurable.
- **FR-009**: When the queue is full, writes MUST be rejected with HTTP
  503 and a `Retry-After` header.

**Service: storage and embeddings**

- **FR-010**: Facts MUST be persisted in Postgres with the columns defined
  in plan.md §4.1 (`id`, `type`, `payload`, `version`, `source`,
  timestamps, `last_used_at`, `use_count`, `confidence`, `decay_weight`)
  and indexes on `(type)`, `(source)`, `(created_at)`, plus a GIN index
  on `payload`.
- **FR-011**: Events MUST be persisted in Postgres in an append-only
  `events` table with the columns defined in plan.md §4.2.
- **FR-012**: Knowledge items MUST be persisted in Qdrant in a collection
  named `knowledge_items` whose payload includes `text`, `source`,
  `tags`, `repo`, `file`, `machine`, and the lifecycle fields listed in
  plan.md §4.3.
- **FR-013**: Knowledge indexing MUST generate embeddings using a
  GPU-accelerated model hosted on `kubs0`; the specific model is selected
  during plan-phase work and recorded in `docs/architecture.md`.
- **FR-014**: Fact upserts MUST deduplicate by `(type, canonical payload)`
  hash; knowledge indexing MUST deduplicate by content hash of `text`.
- **FR-015**: All writes that succeed MUST survive a service restart.

**Service: ops, security, and observability**

- **FR-016**: The service MUST authenticate requests using a bearer token
  loaded from a config file; missing or wrong tokens MUST return 401.
- **FR-017**: The service MUST expose `GET /healthz` returning 200 with a
  JSON body listing the status of each subsystem (Postgres, Qdrant,
  embedding worker, queue) and a non-200 status if any required
  subsystem is down.
- **FR-018**: The service MUST expose `GET /metrics` in Prometheus
  exposition format with at minimum: queue depth, queue capacity, worker
  count, writes accepted total (by type), writes failed total (by type
  and reason), write latency histogram, search latency histogram, and
  embedding latency histogram.
- **FR-019**: The service MUST emit structured logs via `tracing` at
  configurable level; errors MUST include sufficient context to identify
  the failing request and subsystem without exposing the bearer token.
- **FR-020**: The service MUST run as a long-lived process on `kubs0`
  with a documented systemd unit (file shipped under `docs/`); it MUST
  start cleanly on boot when Postgres and Qdrant are available.

**Client crate**

- **FR-021**: A Rust crate `klams-client` MUST be published in-workspace
  exposing typed wrappers for all MVP HTTP endpoints, suitable for use
  by the controller and the viewport's Tauri backend.

**Viewport**

- **FR-022**: The viewport MUST build to a single Windows executable
  named `klams-viewport.exe`; an installer is NOT required for the MVP.
- **FR-023**: The viewport MUST be cross-buildable from Linux via
  `cargo-xwin` (matching the pattern used by `kpidash-client`), and
  the build steps MUST be documented in `viewport/README.md`.
- **FR-024**: The viewport MUST read its connection configuration
  (klams URL and bearer token reference) from
  `%APPDATA%/klams/viewport.toml`; secrets SHOULD be stored via the OS
  credential manager when available, with the TOML holding only a
  reference.
- **FR-025**: The viewport MUST show, on its dashboard, the configured
  service URL, the viewport's own version, and a live connection /
  health indicator that polls `/healthz` at a configurable interval
  (default 10 s).
- **FR-026**: The viewport MUST provide three top-level views — Facts,
  Events, Knowledge — each with the filters and columns defined in
  viewport.md §4 and a detail pane showing the full payload and a
  copy-id action.
- **FR-027**: The Knowledge view MUST issue searches via
  `POST /memory/search` with `types: ["knowledge"]` and show the
  ranked results.
- **FR-028**: When the service is unreachable or returns 4xx/5xx, the
  viewport MUST surface a non-blocking error with enough information to
  diagnose (status code, endpoint, brief message) and MUST NOT retry in
  a tight loop.

**Cross-cutting**

- **FR-029**: The repository MUST contain the Rust workspace layout
  described in plan.md §3.4 (crates: `memory-types`, `memory-core`,
  `memory-store`, `memory-api`, `memory-service`, `klams-client`; plus
  `viewport/` with `src-tauri/` and Svelte frontend). Empty stubs are
  acceptable for crates not exercised by the MVP user stories.
- **FR-030**: `docs/architecture.md`, `docs/setup.md`, and `docs/usage.md`
  MUST be created/updated to reflect the MVP, per constitution
  principle IV.

### Key Entities

- **Fact**: A typed, versioned, structured assertion (User, Task, or
  Env). Carries `payload`, `source`, lifecycle timestamps, and
  confidence/decay fields. Stored in Postgres.
- **Event**: An append-only record of something that happened, scoped
  optionally to a `task_id` and grouped by `category`. Carries
  free-form `payload` and `source`. Stored in Postgres.
- **Knowledge item**: A chunk of text plus metadata, embedded into a
  vector and stored in Qdrant for semantic retrieval. Carries
  lifecycle fields parallel to Fact.
- **Memory write**: The unified pipeline message describing any inbound
  write (fact upsert, event append, knowledge index). Flows through a
  bounded queue to the worker pool.
- **Search result**: A type-tagged, scored hit returned by unified
  search. Includes the source entity's id and a short payload preview.
- **Service health snapshot**: A view of each subsystem's status
  (Postgres, Qdrant, embedding worker, queue) returned by `/healthz`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A controller can record a new user fact and find it again
  via unified search in under 2 seconds on the kubs0 LAN.
- **SC-002**: A knowledge chunk submitted to `/memory/knowledge/index` is
  searchable within 10 seconds at the 95th percentile under MVP load
  (≤ 100 indexing requests per minute).
- **SC-003**: Unified search returns results in under 500 ms at the 95th
  percentile for a corpus of up to 10,000 facts, 50,000 events, and
  10,000 knowledge items.
- **SC-004**: 100% of writes that returned success survive a service
  restart and remain retrievable (verified by an integration test).
- **SC-005**: A malformed write produces an actionable error message
  (names the offending field and the expected shape) in 100% of
  validation-failure cases covered by tests.
- **SC-006**: The viewport, launched on Windows against a populated
  kubs0 instance, shows a green connection indicator and lists facts,
  events, and knowledge items within 3 seconds of launch.
- **SC-007**: From a cold checkout, a developer can build the service
  for Linux and the viewport for Windows (cross-build) using only the
  commands documented in `docs/setup.md` and `viewport/README.md`.
- **SC-008**: `/healthz` correctly reports a non-200 status within 5
  seconds of either Postgres or Qdrant becoming unavailable.
- **SC-009**: Prometheus can scrape `/metrics` and produce a dashboard
  showing queue depth, worker throughput, write latency, and embedding
  latency without any additional service-side code changes.

## Assumptions

- All MVP traffic is local to the homelab LAN; no internet exposure is
  required. Bearer token over plain HTTP on the LAN is acceptable for
  the MVP (TLS is a Phase 5+ concern).
- Postgres on `kubs0` is either dedicated to klams or a shared instance
  with a dedicated `klams` database; the choice is finalized during plan
  phase but does not affect this spec.
- Qdrant is installed natively on `kubs0` (not containerized) per
  plan.md §3.1; on-disk vs in-memory mode is a plan-phase decision.
- The embedding model runs locally on the kubs0 GPU; specific model
  selection (e.g., `bge-small-en-v1.5` vs `nomic-embed-text`) is a
  plan-phase decision and is intentionally not in this spec.
- Decay-aware scoring, schema validation, conflict resolution, and
  hallucination filtering are Phase 2 concerns and are NOT in the MVP.
  Dedupe (FR-014) is included because it prevents storage corruption,
  not because it implements the broader policy layer.
- Ansible callbacks, repo scanners, service monitors, and other
  non-agentic writers (Phase 3) are out of scope for the MVP; only the
  controller and manual `curl`/`klams-client` workflows are MVP writers.
- Backups, Grafana dashboards, `maintenance_mode`, and the restore
  procedure (Phase 5) are out of scope; the MVP only needs to expose
  `/metrics` so Prometheus can scrape it.
- The MCP server for GHCP and other external agents (Phase 6) is out of
  scope.
- The viewport's MVP scope covers the read-only memory inspector
  (viewport.md §4). Write/override actions, context preview, and the
  agent activity panel are out of scope.
- The viewport ships as an unsigned `klams-viewport.exe` binary copied
  to the Windows workstation manually. No installer, no code signing,
  no auto-update in the MVP.
- HTTP/JSON is the API transport for the MVP. gRPC is deferred.
- The repository lives at `github.com/kenhia/klams` and develops on
  `kubs0` once the initial scaffold is in place; `kai` is used only
  for the initial planning round.
