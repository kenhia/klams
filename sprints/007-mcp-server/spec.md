# Feature Specification: MCP Memory Server

**Feature Branch**: `007-mcp-server`
**Created**: 2026-05-24
**Status**: Draft
**Input**: Phase 6 of [sprints/planning/plan.md](../planning/plan.md), informed by [.scratch/author-registration.md](../../.scratch/author-registration.md) and the iteration captured in the planning conversation that preceded this spec.

## Overview

klams currently exposes a typed HTTP API consumed by the controller and the viewport. This feature adds a **Model Context Protocol (MCP) server** so that external agents — GitHub Copilot (GHCP), local controllers, and any future agent — can read and write klams memory using a small, safe public surface.

The MCP server is a **network endpoint** running on `kubs0` alongside `klams-service`. Agents on any machine in the homelab connect to it. Agents are required to **register themselves once per session** before writing, which captures rich authorship metadata (agent name, model, repo, session title, …) so Ken can later analyze "which model produced which memories, and how good were they?"

Write capabilities and destructive operations are gated by **token scopes** (`read` / `write` / `admin`). The everyday agent set has `read` + `write` only — soft-deletes are reversible. A small set of trusted, hardened agents (or Ken himself, via the viewport) hold the `admin` scope and can restore, hard-delete, or list deleted items.

## Clarifications

### Session 2026-05-24

- Q: How does the viewport's `/authors` route read author data — through the MCP server or via REST? → A: New REST endpoints on `klams-service` (viewport stays REST-only); MCP is for external agents.
- Q: What goes into the public `Memory.content` projection per `kind`? → A: Return the *memory itself*, not internal bookkeeping. Drop `version`, `confidence`, `decay_weight`, `use_count`, `last_used_at`, raw embedding vectors, and the internal `source` trust tier. Keep the user-meaningful payload, tags, author subset, and timestamps. Per-kind shape recorded in the **Key Entities** section.
- Q: What is the `~/.copilot`-equivalent registration format Ken needs to copy-paste? → A: Document **two** environments, both shipped in `docs/setup.md`: VS Code / VS Code Insiders uses `<workspace>/.vscode/mcp.json` with top-level `servers` key; GHCP CLI uses `~/.copilot/mcp-config.json` with top-level `mcpServers` key. Both support `type: "http"` entries with a `url` and `headers` (for the bearer token).

## User Scenarios & Testing *(mandatory)*

### User Story 1 — GHCP records a learned fact mid-session (Priority: P1)

While working in a VS Code Copilot Chat session, GHCP discovers a durable preference ("Ken prefers `just` over `make` for new repos"). It registers itself with klams, writes the fact, and the controller running on another machine can find it minutes later.

**Why this priority**: This is the primary "use klams from an agent" loop and the whole point of Phase 6. Without it, klams remains a controller-only system.

**Independent Test**: Run a scripted MCP client against a live klams-mcp endpoint: call `register_author`, then `memory_add`, then from a *separate* client call `memory_search` and confirm the fact comes back with the registering author's metadata attached.

**Acceptance Scenarios**:

1. **Given** klams-mcp is running and the agent holds a `write`-scoped token, **When** the agent calls `register_author` with `{agent_name: "GHCP", model: "claude-opus-4.7", repo: "/home/ken/src/ai/klams", session_title: "Phase 7 design"}`, **Then** the server returns a fresh `author_id` (UUID) and records the metadata.
2. **Given** a valid `author_id`, **When** the agent calls `memory_add` with `{author_id, kind: "fact", content: {...UserFact payload...}, tags: ["preferences", "tooling"]}`, **Then** the server enqueues the write through the existing klams pipeline, returns the created `memory_id`, and the row carries the author FK.
3. **Given** the fact has been written, **When** a second MCP client calls `memory_search` with a query covering the topic, **Then** the fact appears in the results and the response includes the author's `agent_name` and `model`.
4. **Given** an agent calls `memory_add` without first calling `register_author`, **When** the server processes the request, **Then** it returns an MCP error `MISSING_AUTHOR_ID` with a remediation hint and does **not** write anything.

---

### User Story 2 — Agent retrieves context before answering (Priority: P1)

GHCP (or any agent) receives a user question and, before answering, queries klams for relevant facts and knowledge. Results are deduplicated across kinds and capped at a token budget. The agent has zero knowledge of klams' internal taxonomy — it just asks for "memory."

**Why this priority**: Reads are the higher-volume side of the workload. If reads don't work cleanly, agents won't bother writing.

**Independent Test**: With a fixture-loaded klams, invoke `memory_search` from an MCP client with a natural-language query and confirm a unified result list of `Memory{kind, content, tags, author, …}` items is returned, ordered by relevance, with no internal-only fields (decay_weight, raw embedding vector, internal version numbers).

**Acceptance Scenarios**:

1. **Given** klams contains facts and knowledge items relevant to a query, **When** an agent calls `memory_search` with `{query: "...", top_k: 10}`, **Then** the server returns a list of `Memory` items each tagged with `kind: "fact" | "knowledge" | "event"`, sorted by relevance, with no internal fields leaked.
2. **Given** a memory item ID returned from search, **When** the agent calls `memory_related(id, top_k: 5)`, **Then** the server returns up to 5 semantic neighbors of that item (drawn from the same and adjacent kinds), excluding the original.
3. **Given** an agent passes `kinds: ["fact"]`, **When** `memory_search` runs, **Then** only fact items are returned.

---

### User Story 3 — Agent records a deployment event (Priority: P2)

A controller-driven agent ran `just deploy widget` on `kub3` and wants the event recorded in klams so future searches surface it ("when was widget last deployed?").

**Why this priority**: Events are how agents leave breadcrumbs. They unlock the "what did this agent actually do?" review loop and the per-author analytics in US5.

**Independent Test**: Call `memory_append_event` with a `category` and `payload`, then call `memory_search` with `kinds: ["event"]` and confirm the event is returned with the author FK populated.

**Acceptance Scenarios**:

1. **Given** a registered author, **When** the agent calls `memory_append_event({author_id, category: "Deploy", payload: {service: "widget", host: "kub3", version: "..."}, task_id: null})`, **Then** the server appends to the `events` table with the author FK populated.
2. **Given** a recent deploy event, **When** an operator queries `memory_search({query: "widget deploy", kinds: ["event"], top_k: 5})`, **Then** the event appears with author attribution.

---

### User Story 4 — Agent makes a mistake and the system recovers (Priority: P2)

A small/aggressively-quantized agent hallucinates and calls `memory_delete` on a stack of valuable items it didn't actually own. Because deletes are soft by default, nothing is lost. Ken (or a hardened admin agent) reviews the deleted set in the viewport and restores them.

**Why this priority**: Without this, a single bad agent run could destroy the memory store. This is the primary safety story for letting less-trusted agents have `write` scope.

**Independent Test**: With known fixture rows, call `memory_delete(id)` from a `write`-scoped token; confirm subsequent `memory_search` no longer returns the row; then with an `admin`-scoped token, call `memory_admin_list_deleted` (the row appears) and `memory_admin_restore(id)`; confirm `memory_search` returns the row again.

**Acceptance Scenarios**:

1. **Given** a memory item exists and is visible in search, **When** any `write`-scoped agent calls `memory_delete(id)`, **Then** the row's `deleted_at` and `deleted_by_author_id` are populated and subsequent default `memory_search` calls do **not** return it.
2. **Given** a soft-deleted item, **When** a `read` or `write` token attempts `memory_admin_restore(id)` or `memory_admin_hard_delete(id)`, **Then** the call fails with `INSUFFICIENT_SCOPE` and the row is unchanged.
3. **Given** a soft-deleted item, **When** an `admin`-scoped token calls `memory_admin_restore(id)`, **Then** `deleted_at` is cleared and the item reappears in `memory_search`.
4. **Given** a soft-deleted item, **When** an `admin`-scoped token calls `memory_admin_hard_delete(id)`, **Then** the row is removed from Postgres / Qdrant and is no longer enumerable via `memory_admin_list_deleted`.
5. **Given** the `tools/list` MCP request, **When** a `read`-only token calls it, **Then** only read-classified tools are returned; `write` tokens additionally see write tools; `admin` tokens see everything.

---

### User Story 5 — Ken reviews per-author activity (Priority: P3)

Ken wants to see "which agents/models wrote what, and what fraction of their proposals got rejected or deleted?" both in Grafana (aggregate) and in the viewport (drilldown).

**Why this priority**: Closes the analytics loop that motivates author registration. Not needed for first-light use of klams from agents, but unlocks the long-term "grade my agents" workflow.

**Independent Test**: Drive several `memory_add`, `memory_delete`, and dissent operations from multiple author profiles. Confirm `klams_mcp_writes_total{author_name,model,kind,outcome}` and `klams_mcp_deletes_total{author_name,model,mode}` increment correctly in `/metrics`. Confirm the viewport `/authors` route lists each author with recent activity counts.

**Acceptance Scenarios**:

1. **Given** writes from multiple registered authors, **When** Prometheus scrapes `/metrics`, **Then** counters are labeled by `author_name` and `model` (not by `author_id`, to bound cardinality) and reflect the activity.
2. **Given** the viewport is configured against klams, **When** Ken opens `/authors`, **Then** he sees one row per author with `agent_name`, `model`, `session_title`, last-seen time, and counts (writes, soft-deletes, restored).
3. **Given** Ken clicks an author row in the viewport, **When** the detail view loads, **Then** he sees a paginated list of memory items authored by that registration with their current state (live / soft-deleted / hard-deleted).

---

### Edge Cases

- **Concurrent authors with identical metadata**: Two GHCP sessions in two VS Code windows on the same repo. Both register independently and receive distinct `author_id`s; no deduplication is attempted on registration metadata.
- **Long-running session crosses days**: An author registered weeks ago is still valid; the server stamps `last_seen_at` on every call but never expires authors. Stale authors are an analytics concern, not a security one.
- **Agent provides an `author_id` that doesn't exist** (e.g., klams was restored from backup mid-session): server returns `UNKNOWN_AUTHOR_ID`; agent's contract is to call `register_author` again and retry.
- **Concurrent soft + hard delete race**: hard-delete on an already-hard-deleted row returns `NOT_FOUND`; soft-delete on an already-soft-deleted row is idempotent (no-op, success).
- **Maintenance window** (sprint 006): MCP write tools return the same `503 + maintenance_window_active` envelope as the REST API; reads continue to serve.
- **Token scope downgrade after issue**: rotating a token from `admin` to `write` immediately strips admin tools from `tools/list` for new sessions; in-flight admin calls complete or are rejected at the next request.
- **MCP client doesn't support Streamable HTTP yet** (e.g., older GHCP build): fall back to HTTP+SSE on the same endpoint family if the negotiated transport requires it.
- **Author registration spam**: an agent calling `register_author` thousands of times produces many author rows. No enforcement in v1; the `klams_mcp_writes_total` counters and an "authors created per hour" panel surface the pattern for follow-up.
- **Embedding model unavailable**: `memory_add` with `kind: "knowledge"` fails with a retryable error; the agent retries; the existing klams-core retry/backoff for the TEI adapter applies.

## Requirements *(mandatory)*

### Functional Requirements

**MCP transport and discovery**

- **FR-001**: The system MUST expose an MCP server over a network transport so that agents on any homelab host can connect to it. The transport MUST be one of the MCP-standard remote transports (Streamable HTTP preferred; HTTP+SSE as fallback if a target client lacks Streamable HTTP support).
- **FR-002**: The MCP server MUST run as part of `klams-service` and share its tokio runtime, configuration loader, and lifecycle (including the sprint 006 maintenance window).
- **FR-003**: The MCP server MUST advertise tool definitions via the standard MCP `tools/list` mechanism, filtered by the calling token's scopes (see FR-020).

**Authorship**

- **FR-004**: The system MUST provide a `register_author` tool that accepts agent metadata and returns a server-generated `author_id` (UUID). At minimum, the metadata fields are: `agent_name` (required), `model`, `session_title`, `repo`, `client_app`, `client_version`, plus an open `extra` JSON object.
- **FR-005**: The system MUST persist authors in a dedicated table with `created_at` and `last_seen_at` timestamps; `last_seen_at` MUST be updated by the server on every authenticated MCP call that references the author.
- **FR-006**: All MCP write tools (`memory_add`, `memory_append_event`, `memory_delete`) MUST require an `author_id` argument and MUST return `MISSING_AUTHOR_ID` if it is absent and `UNKNOWN_AUTHOR_ID` if it does not exist.
- **FR-007**: Every memory row written via MCP (facts, events, and Qdrant knowledge payloads) MUST carry the `author_id` as a foreign key or payload field. Pre-MCP rows and non-MCP writes (controller, ansible, scanners) MUST be backfilled to reference a single reserved "system" author so the column is non-null and joins always succeed.

**Public memory surface**

- **FR-008**: The system MUST expose a single `Memory` projection over the wire with a `kind` discriminator (`"fact" | "knowledge" | "event"`) and a small, stable set of fields. Internal-only fields (raw embedding vector, `decay_weight`, optimistic-concurrency `version`, raw event-source taxonomy, internal IDs unrelated to the row's UUID) MUST NOT appear in any MCP response.
- **FR-009**: The system MUST provide a `memory_add` tool that accepts `{author_id, kind: "fact"|"knowledge", content, tags?}`. Embeddings for knowledge items MUST be computed server-side; clients MUST NOT supply or override the embedding model.
- **FR-010**: The system MUST provide a `memory_append_event` tool that accepts `{author_id, category, payload, task_id?}` and appends to the `events` table.
- **FR-011**: The system MUST provide a `memory_search` tool that accepts `{query, kinds?, tags?, top_k?, filters?}` and returns a unified, relevance-ordered list of `Memory` items across the requested kinds, hiding soft-deleted rows by default.
- **FR-012**: The system MUST provide a `memory_related` tool that accepts `{id, top_k?}` and returns semantic neighbors of the referenced item, excluding the item itself and soft-deleted rows.

**Deletion and admin**

- **FR-013**: `memory_delete(id)` MUST be a **soft delete**: it sets `deleted_at` (UTC timestamp) and `deleted_by_author_id` on the row and removes it from default search visibility. It MUST NOT remove the row from Postgres or Qdrant.
- **FR-014**: Soft delete MUST be idempotent: a second `memory_delete` on an already-deleted row succeeds without modifying `deleted_at` or `deleted_by_author_id`.
- **FR-015**: Events MUST NOT be deletable via MCP (they are append-only); `memory_delete` on an event ID MUST return `EVENTS_NOT_DELETABLE`.
- **FR-016**: The system MUST provide three admin tools that require the `admin` scope: `memory_admin_restore(id)` (clears `deleted_at`), `memory_admin_hard_delete(id)` (removes the row from Postgres and Qdrant), and `memory_admin_list_deleted({kinds?, since?, author_id?, limit?})`.

**Authorization**

- **FR-017**: The system MUST extend the existing bearer-token auth to support multiple tokens, each tagged with a set of scopes drawn from `{read, write, admin}`.
- **FR-018**: The existing single-token configuration form (`[auth] bearer_token = "…"`) MUST continue to work and be treated as a token with all three scopes, so no operator change is required for non-MCP REST clients.
- **FR-019**: Tokens MUST be compared in constant time, as today, and MUST be configurable without code changes.
- **FR-020**: At MCP `tools/list` time, the server MUST return only the tools whose required scope is held by the calling token. At tool-call time, the server MUST re-validate scope and return `INSUFFICIENT_SCOPE` if the call exceeds the token's grants.

**Maintenance and operational integration**

- **FR-021**: During the sprint 006 maintenance window, MCP write tools MUST return a `503`-equivalent MCP error carrying `Retry-After` semantics and the `MAINTENANCE_WINDOW_ACTIVE` discriminator; reads MUST continue to serve.
- **FR-022**: The MCP server MUST expose Prometheus counters: `klams_mcp_writes_total{agent_name, model, kind}`, `klams_mcp_deletes_total{agent_name, model, mode}` where `kind ∈ {"fact","knowledge","event"}` and `mode ∈ {"soft","restored","hard"}`, and `klams_mcp_search_total{agent_name, model}`. Labels MUST NOT include `author_id` (cardinality), and the search counter MUST NOT use the request's `kinds` set as a label (combinatorial cardinality).
- **FR-023**: All MCP requests MUST be traced through the existing `tracing` stack with `author_id`, `agent_name`, `model`, and tool name attached as span fields for offline correlation.

**Viewport**

- **FR-024**: The viewport MUST gain an `/authors` route showing one row per registered author (`agent_name`, `model`, `session_title`, `repo`, `last_seen_at`, write count, soft-delete count, restore count). The route MUST read its data from REST endpoints on `klams-service`, not from the MCP server.
- **FR-024a**: `klams-service` MUST expose the following REST endpoints, gated by `read` scope, to back the viewport: `GET /v1/authors` (paginated list with activity counts), `GET /v1/authors/{id}` (single author with full metadata), and `GET /v1/authors/{id}/memories` (paginated list of memory items authored by that registration, with current state).
- **FR-025**: The viewport `/authors` detail view for a given author MUST list memory items they authored, indicating current state (live / soft-deleted / hard-deleted) and supporting follow-the-link navigation to the existing facts/knowledge/events routes.

**Documentation and registration**

- **FR-026**: `docs/setup.md` and `docs/usage.md` MUST gain an MCP chapter covering: registering the server with a client, the tool surface, token-scope configuration, and the soft-delete safety model. Registration coverage MUST include copy-pasteable snippets for both **VS Code / VS Code Insiders** (`<workspace>/.vscode/mcp.json`, top-level `servers` key) and the **GHCP CLI** (`~/.copilot/mcp-config.json`, top-level `mcpServers` key). Each snippet MUST use `type: "http"` with a `url` pointing at the klams-mcp endpoint and an `Authorization: Bearer` header carrying the configured token.
- **FR-027**: `docs/architecture.md` MUST gain a §2e block describing the MCP projection layer, the authors table, and the scope-gated tool surface.

### Key Entities

- **Author**: A single registered session of an external agent. Attributes: `id` (UUID, server-issued), `agent_name`, `model`, `session_title`, `repo`, `client_app`, `client_version`, `extra` (JSON), `created_at`, `last_seen_at`. Referenced by every memory row written via MCP. A single reserved "system" author covers non-MCP writes.
- **Memory (public projection)**: The over-the-wire shape returned by `memory_search`, `memory_related`, and `memory_add`. Common envelope: `id` (UUID), `kind` (`"fact" | "knowledge" | "event"`), `tags` (list), `author` (subset of author fields: `agent_name`, `model`, `repo`), `created_at`, `updated_at`. Per-kind `content`:
  - **fact** — `{type, payload}` where `type` is the user-facing fact type (`UserFact`, `TaskFact`, `EnvFact`, …) and `payload` is the typed body the agent originally wrote.
  - **knowledge** — `{text, source_path?, repo?}` — the indexed text and its origin.
  - **event** — `{category, payload, task_id?}` — the event taxonomy the agent originally wrote.

  Deliberately **omitted** from every projection: raw embedding vectors, `decay_weight`, `confidence`, `use_count`, `last_used_at`, optimistic-concurrency `version`, the internal `source` trust tier, and any internal identifiers unrelated to the row's public UUID.
- **Token Scope Grant**: A configured association of a bearer token with a set of scopes from `{read, write, admin}`. Persisted only in `klams.toml`; never in the database.
- **Soft Deletion State**: Two columns added to `facts` and to the Qdrant knowledge payload: `deleted_at` (nullable UTC timestamp) and `deleted_by_author_id` (nullable UUID FK). `events` are append-only and never carry these fields.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: GHCP, running in VS Code on a non-`kubs0` host, can complete the full loop — `register_author` → `memory_add` → restart VS Code → `memory_search` retrieves the item — without manual intervention.
- **SC-002**: 100% of memory rows written via the MCP server carry a non-null `author_id` linked to a real `authors` row. Verified by a startup invariant check.
- **SC-003**: A soft-deleted memory item is restorable to its original content and tags using only `memory_admin_restore`, with no data loss.
- **SC-004**: A `read`-only token receives only read tools from `tools/list` and gets `INSUFFICIENT_SCOPE` on any write or admin tool call. Verified by an integration test.
- **SC-005**: Grafana's "MCP author activity" panel shows write and delete rates broken down by `agent_name` and `model` after at least one MCP session of each kind.
- **SC-006**: A representative `memory_search` from an MCP client returns a unified result set under the configured token budget in under one second (p95 over a 100-call sample) at the homelab's typical store size (≤ 10k facts, ≤ 50k knowledge items). **Note**: any measured p95 above the 1 s boundary is to be reviewed with the user before any tuning work is undertaken; modest overshoot is likely "good enough" for the homelab.
- **SC-007**: `docs/setup.md` contains copy-pasteable sections for both VS Code/VS Code Insiders (`<workspace>/.vscode/mcp.json`) and the GHCP CLI (`~/.copilot/mcp-config.json`) that take Ken from "no MCP configured" to a working integration in under 15 minutes for either environment.
- **SC-008**: A simulated "rogue agent" run that issues 100 spurious `memory_delete` calls leaves every item soft-deleted and 100% restorable; no row is lost from Postgres or Qdrant.

## Assumptions

- The MCP **2025-spec Streamable HTTP** transport is the target; an HTTP+SSE fallback is acceptable if the chosen MCP client (initially GHCP, May 2026 build) does not yet support Streamable HTTP. The choice between them is a research item for the plan phase, not a scope change.
- The MCP server runs **in the same `klams-service` process** as the REST API. A separate binary is out of scope for this sprint.
- Rate limiting and per-agent quotas are **deferred**. Grafana telemetry surfaces abnormal patterns; enforcement waits until the data justifies it.
- The viewport `/authors` route is included in sprint 007. The pre-existing viewport routes (`/facts`, `/knowledge`, `/events`, `/dissents`, `/preview`) are unchanged.
- Token scopes (`read` / `write` / `admin`) cover every authorization concern for this sprint. Finer-grained scopes (per-kind, per-author) are out of scope.
- The MCP tool naming convention is snake_case (e.g., `memory_add`, not `memory.add`) because MCP tool names typically forbid dots.
- The "system" backfill author is a single fixed UUID baked into a migration. Different non-MCP source types (`User`, `Controller`, `Task`) are still distinguished by the existing `source` column on `facts` and `events`; `author_id` is an orthogonal axis carrying *rich* attribution.
- Sprint 006's `MaintenanceState` is reused as-is to gate MCP writes; no new maintenance-window plumbing is needed.
- Embedding generation continues to use the existing TEI adapter; the MCP server never receives or stores client-supplied vectors.
- Soft-delete columns are added to `facts` and to the Qdrant knowledge payload only. `events` remain append-only.
- MCP client registration formats and locations are covered by Clarifications Q3 and shipped in `docs/setup.md`; klams does not write into either client config file automatically.

## Dependencies

- Sprint 006 (maintenance window, status hook, backups) is shipped — see commit `782bcd9`. The MCP server depends on `MaintenanceState` from that sprint.
- `klams-types`, `klams-core`, `klams-store`, `klams-api`, `klams-service`, `viewport/` — all existing crates and packages remain in their current form and gain additive changes only.
- An MCP Rust SDK (or hand-rolled JSON-RPC handler) — selection deferred to the plan phase.
