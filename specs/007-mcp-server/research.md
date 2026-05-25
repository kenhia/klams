# Research — MCP Memory Server (sprint 007)

This document resolves Technical Context unknowns and records design
decisions referenced from [plan.md](./plan.md). Each entry follows
**Decision / Rationale / Alternatives** so future readers can audit
the trade space.

---

## R-001 — Choice of MCP transport (Streamable HTTP vs HTTP+SSE)

**Decision**: Implement **Streamable HTTP** as the primary transport, and ship **HTTP+SSE** on the same axum mount as a fallback when the client's `Accept`/handshake negotiation indicates SSE-only support. Both transports share one request handler and one auth middleware.

**Rationale**:

- The MCP 2025-03 specification deprecates the standalone HTTP+SSE pattern in favor of Streamable HTTP (single endpoint, optional upgrade to event-stream on the response side, no separate POST/GET split). All new SDK work is targeting Streamable HTTP.
- GitHub Copilot's `.vscode/mcp.json` "type: http" form (already used by Ken with the `github` server at `https://api.githubcopilot.com/mcp/` — see `.vscode/mcp.json`) is documented as Streamable HTTP. Same shape for the GHCP CLI's `mcp-config.json`.
- Some older or third-party clients still expect HTTP+SSE (separate `POST /messages` + `GET /sse`). Falling back on the same mount costs ~20 lines of glue (route detection by `Accept: text/event-stream`) and avoids cutting off non-GHCP agents that Ken may add later.
- Bundling both is cheap because the underlying tool-dispatch logic is transport-agnostic — only the framing layer differs.

**Alternatives considered**:

- **Streamable HTTP only**: Risk locking out future agents that lag the spec. Rejected — fallback is nearly free.
- **HTTP+SSE only**: Aligns with older docs but is on the deprecation path; would force a v2 transport rework before sprint 008. Rejected.
- **stdio**: Already excluded in spec.md "Assumptions" — requires a server-per-agent-host process tree and cannot reach kubs0-centralized klams.

---

## R-002 — Rust MCP SDK selection

**Decision**: Use the **`rmcp` crate** (official Rust SDK from `modelcontextprotocol/rust-sdk`) pinned at **`1.7.0`** with features `["server", "macros", "schemars", "transport-streamable-http-server"]` in `crates/klams-mcp/Cargo.toml`. Verified at T001 on 2026-05-24: rmcp 1.7.0 ships the `transport-streamable-http-server` feature exposing `StreamableHttpService`, which is tower-compatible and mounts cleanly on the existing axum 0.7 router. SDK MSRV is 1.92; our installed toolchain is 1.94.1 (workspace `rust-toolchain.toml` channel = `stable`) so no MSRV bump is required (the `rust-version = "1.83"` field in workspace `Cargo.toml` is the published MSRV for downstream consumers, not the build floor).

**Rationale**:

- Maintained by the MCP project; tracks spec updates including Streamable HTTP framing.
- Exposes a tower-compatible service that mounts cleanly on the existing axum router stack — same `Router::nest()` pattern we already use for `/healthz` vs protected routes.
- JSON-RPC envelope, content-type negotiation, capability advertisement, and `tools/list`/`tools/call` plumbing are provided; we only own the tool implementations and the schema declarations.
- Hand-rolling an MCP server would require maintaining wire-format compliance ourselves and would slow Phase 7 (additional MCP-driven features) for no gain.

**Alternatives considered**:

- **Hand-rolled JSON-RPC over axum**: ~500 LOC of envelope/dispatch we'd have to keep aligned with the spec. Rejected.
- **`mcp-rs` (community fork)**: Smaller user base, less spec coverage. Rejected absent a concrete `rmcp` blocker.
- **Implement via WASM bridge to TypeScript SDK**: Adds two languages and a runtime; violates Principle VI. Rejected.

**T001 checkpoint result (2026-05-24)**: rmcp 1.7.0 confirmed to ship `StreamableHttpService` under feature `transport-streamable-http-server`. No fallback needed. Tool definitions are declared via `#[tool_router]` + `#[tool]` macros; the JSON Schema 2020-12 fixtures in `contracts/tool-schemas/` will be cross-checked against the `schemars`-generated schemas in an integration test.

---

## R-003 — Soft-delete representation in Qdrant

**Decision**: Add **two payload fields** to existing knowledge-item points: `deleted_at` (ISO-8601 string, omitted when not deleted) and `deleted_by_author_id` (UUID string, omitted when not deleted). Default search adds a Qdrant filter `must_not: [ {key: "deleted_at", match: {except: null}} ]` (i.e., exclude points with the field present).

**Rationale**:

- One collection, zero schema migration in Qdrant — payload fields are schemaless.
- Filter cost is O(1) per candidate and Qdrant indexes payload fields lazily; for the homelab corpus size (~50k items target) the overhead is negligible.
- Mirrors the Postgres `facts.deleted_at IS NULL` filter so service-layer code can share a single "live filter" abstraction.
- `memory_admin_list_deleted` is implemented as the inverse filter (`must: [{key:"deleted_at", ...}]`).
- Hard delete uses Qdrant's existing point-delete-by-id API; no extra bookkeeping.

**Alternatives considered**:

- **Separate `knowledge_items_deleted` collection**: Doubles operational surface (backups, snapshots, retention) for a feature that doesn't need physical isolation. Rejected.
- **Move deleted points out of Qdrant into a Postgres "tombstone" table**: Loses the embedding for restore (would have to re-embed on restore). Rejected — restore must be byte-exact.
- **No soft delete in Qdrant; hard delete only**: Breaks the "rogue agent doesn't lose data" safety story (SC-008). Rejected.

---

## R-004 — Multi-token bearer auth with constant-time comparison

**Decision**: Refactor `AuthState` in `klams-api` from a single `Arc<Vec<u8>>` expected token to `Arc<Vec<TokenGrant>>` where `TokenGrant { token_bytes: Vec<u8>, scopes: ScopeSet }`. Middleware iterates **all** grants, `ct_eq`s the candidate against each `token_bytes`, and returns the first match's `ScopeSet` (or `Unauthorized`). The iteration is **deliberately unconditional** — the loop runs over every configured token regardless of early matches, preserving the constant-time property across the token set.

`ScopeSet` is a packed `u8` with bits for `read | write | admin`; a "legacy" `bearer_token` single-string config form is materialized at load time into one grant with all three bits set.

**Rationale**:

- Preserves the existing constant-time guarantee (the dominant timing signal is "token of length N matches *some* configured token" — we don't leak which one via early exit).
- Adds <30 LOC delta to the auth module; config loader changes contained in `klams-types::AuthConfig`.
- Scope check on each MCP tool call is a single `scopes.contains(required)` bitwise op — no allocation, no lock.
- Backward-compatible: existing `[auth] bearer_token = "..."` configurations remain valid.

**Alternatives considered**:

- **HashMap keyed by token**: O(1) lookup but reveals "token exists" via timing if any branch differs (e.g., scope check). Constant-time-equality + linear scan is safer and equally fast at single-digit-token counts. Rejected.
- **Per-route scope guards via custom extractor**: Pushes scope checking out of middleware into each handler. More surface for mistakes; harder to keep MCP `tools/list` filtering in sync. Rejected.
- **External auth (JWT, OAuth)**: Out of scope per spec assumption + Principle VI. Rejected.

---

## R-005 — Author identity binding to bearer tokens

**Decision**: `author_id` is **not bound to the bearer token** that created it. Any token with the `write` scope can submit any existing `author_id` on a write call. The `authors` table is treated as a shared registry of authorship metadata; the token's scope is the only gate on whether the call is allowed at all.

**Rationale**:

- klams has a single human user (Ken). All tokens ultimately represent agents Ken has authorized. No multi-tenant boundary exists to defend.
- Binding `author_id` to a token would force agents to re-register on every token rotation (operationally annoying) and would block a legitimate "long-lived agent re-uses an `author_id` after a service restart" workflow.
- Audit trail is still preserved via the `tracing` spans (FR-023) — every MCP call logs token-hash + author_id, so post-hoc correlation of "which token submitted on behalf of which author" is possible.

**Alternatives considered**:

- **Per-token author allow-list in config**: Complicates registration; agents would have to coordinate with klams config before first use. Rejected.
- **Server-issued opaque session tokens that combine auth + author**: Conflates two axes; loses the long-lived-attribution benefit of separate authors. Rejected.

---

## R-006 — Soft-delete columns and event append-only invariant

**Decision**: Add `deleted_at TIMESTAMPTZ NULL` and `deleted_by_author_id UUID NULL` to the **`facts`** table and to the Qdrant knowledge-item payload. The **`events`** table receives **neither** — events remain strictly append-only, and `memory_delete` on an event ID returns `EVENTS_NOT_DELETABLE` as required by FR-015.

**Rationale**:

- Events represent things that happened; rewriting history (even softly) breaks their value for audit and review. Consistent with the existing append-only `events` schema.
- If an event was written in error, the correction path is "write a corrective event"; no MCP tool surfaces a delete path for events at all.
- Soft delete on facts/knowledge preserves the restore-from-rogue-agent property (SC-008) without giving the same power to rewrite event history.

**Alternatives considered**:

- **Soft delete on all three kinds for symmetry**: Tempting but breaks the events contract. Rejected.
- **Allow admin-only hard delete on events**: Marginal value; adds a new tool with an extra failure mode. Rejected — if an event must be removed, it can be done via the Postgres CLI as an out-of-band operator action, with no client API path.

---

## R-007 — Backfill author for pre-MCP rows

**Decision**: Migration `0005_authors.sql` (or its sqlx-migrate equivalent) creates the `authors` table and inserts **one** row with a fixed UUID and `agent_name = "system"`, `model = NULL`, `client_app = "klams-service"`, etc. The migration then `UPDATE`s every existing `facts.author_id` and `events.author_id` to this UUID. The migration is idempotent (`INSERT ... ON CONFLICT DO NOTHING` on the system author UUID).

**Rationale**:

- Satisfies FR-007's "non-null" requirement without inventing a per-source-tier sentinel set (User/Controller/Task/AgentProposal could each have an author, but that's tracked by the existing `source` column — duplicating it as authors would be redundant).
- Future analytics queries always join cleanly because `author_id` is never NULL.
- Fixed UUID is a constant in `klams-types::SYSTEM_AUTHOR_ID` so tests, fixtures, and analytics can reference it by name.

**Alternatives considered**:

- **NULLABLE `author_id` with `IS NULL` meaning "system"**: Forces every query to handle the NULL case. Rejected.
- **One backfill author per existing `Source` value**: Conflates the orthogonal axes the spec keeps separate. Rejected.

---

## R-008 — REST endpoints for viewport author drilldown

**Decision**: Add three new REST routes under the existing `klams-api` protected mount, all requiring the `read` scope:

- `GET /v1/authors?limit=&cursor=` — paginated list with activity counts via SQL aggregations.
- `GET /v1/authors/{id}` — single author detail.
- `GET /v1/authors/{id}/memories?limit=&cursor=&kinds=&state=` — paginated memories authored by this registration. `state` is `live | deleted | hard_deleted_traces | all`. (`hard_deleted_traces` is empty in v1 — we don't keep tombstones for hard deletes — but the param shape is forward-compatible.)

**Rationale**:

- Keeps the viewport on the REST contract it already uses (`klams-client` crate). No MCP knowledge required in the viewport.
- Reuses the existing auth middleware; only the legacy single-token config grants the viewport's bearer the `read` scope.
- The cursor pagination shape matches existing list endpoints (`/v1/facts`, `/v1/knowledge`) — see contract test parity in `crates/klams-api/tests/contract_*.rs`.

**Alternatives considered**:

- **Single endpoint with embedded memories**: Couples list view to detail view; breaks the existing pagination story. Rejected.
- **GraphQL slice**: Overkill for three endpoints; adds dependency. Rejected.

---

## R-009 — Maintenance window integration

**Decision**: Reuse `MaintenanceState` (from sprint 006) directly. Every MCP write tool checks `maintenance.is_active()` at entry and returns the MCP equivalent of the REST `503 + Retry-After + {"error":"maintenance_window_active"}` envelope:

```json
{
  "isError": true,
  "content": [{"type": "text", "text": "Maintenance window active; retry after <seconds>s"}],
  "_meta": {"error_code": "MAINTENANCE_WINDOW_ACTIVE", "retry_after_seconds": 30}
}
```

(MCP tool errors use the `isError: true` content envelope; we encode the `Retry-After` semantics in `_meta` for clients that want machine-readable hints.)

**Rationale**:

- Zero net-new plumbing; the existing `CriticalWrite` extension marker on the REST router translates 1:1 to "write-scoped MCP tools".
- Reads continue to serve, matching the REST behavior.

**Alternatives considered**:

- **Block all MCP calls during maintenance** (including reads): Inconsistent with REST behavior. Rejected.
- **Custom maintenance state for MCP**: Two sources of truth. Rejected.

---

## R-010 — Cardinality discipline for Prometheus author labels

**Decision**: MCP counters use `agent_name` and `model` as labels on every counter (low cardinality — handful of agent classes, handful of model SKUs, matching the `authors.agent_name` column name). Two counters carry one additional **bounded** label: `klams_mcp_writes_total` adds `kind ∈ {fact, knowledge, event}` (3 values), and `klams_mcp_deletes_total` adds `mode ∈ {soft, restored, hard}` (3 values). `klams_mcp_search_total` carries `agent_name` + `model` only — the request's `kinds` set is **not** a label because its power-set is combinatorial. **Never** `author_id`. For per-author detail, query Postgres directly via the viewport `/authors/{id}/memories` endpoint (R-008). This decision is the authoritative label set; FR-022 in spec.md mirrors it.

**Rationale**:

- An unbounded `author_id` label set would blow Prometheus's series budget within weeks of normal use.
- `kind` and `mode` are tiny closed enums; they give Grafana the breakdown that motivates author analytics without exploding cardinality.
- The author registry is small enough (target O(100) total over the system's lifetime) that direct DB queries are cheap when richer drilldown is needed.

**Alternatives considered**:

- **Add a `kinds` label to the search counter**: power-set of `{fact,knowledge,event}` plus per-call subset selection blows cardinality unnecessarily. Rejected.
- **Hash author_id to N buckets**: Lossy and surprising. Rejected.
- **Per-author histogram in a separate /metrics endpoint**: Over-engineering for the homelab scale. Rejected.

---

## R-011 — Tool naming convention

**Decision**: All MCP tools use **snake_case** identifiers without dots: `register_author`, `memory_add`, `memory_search`, `memory_related`, `memory_delete`, `memory_append_event`, `memory_admin_restore`, `memory_admin_hard_delete`, `memory_admin_list_deleted`.

**Rationale**:

- MCP tool name validation in most SDK implementations restricts to `[a-zA-Z0-9_-]+`; dots cause registration failures.
- `memory_admin_*` prefix groups admin tools visually in `tools/list` output.
- Matches the verb-first convention used by other MCP servers Ken has registered (`kwi-mcp`, `kpidash-mcp`).

**Alternatives considered**:

- **Dotted namespace (`memory.add`)**: Spec-illegal in some clients. Rejected.
- **Camel-case (`memoryAdd`)**: Inconsistent with the homelab's existing MCP servers. Rejected.

---

## R-012 — Embedding model and embedding-on-write path

**Decision**: `memory_add` for `kind: "knowledge"` enqueues a `MemoryWrite::KnowledgeIndex` exactly as the REST path does today; the existing TEI adapter computes the embedding in the worker pool. Clients **never** supply or override embeddings.

**Rationale**:

- Embedding model identity is a klams-deployment decision, not an agent decision. Letting agents supply vectors would let a misbehaving agent poison search results with random vectors.
- Reusing the existing pipeline means the same retry/backoff, dedupe, and validation paths apply.
- No new code in `klams-store`; only the MCP-facing `memory_add` handler differs from the REST handler in input shape.

**Alternatives considered**:

- **Allow client-supplied embeddings with a flag**: Trust boundary violation. Rejected.

---

## Summary of resolved unknowns

| Spec assumption | Research entry | Status |
|-----------------|----------------|--------|
| Streamable HTTP vs HTTP+SSE | R-001 | Resolved — both, primary Streamable HTTP |
| Rust MCP SDK | R-002 | Resolved — `rmcp` with checkpoint at T001 |
| Qdrant soft-delete representation | R-003 | Resolved — payload fields + filter |
| Token-scope auth implementation | R-004 | Resolved — constant-time linear scan |
| Author/token binding | R-005 | Resolved — unbound |
| Soft delete on events | R-006 | Resolved — never |
| Backfill strategy | R-007 | Resolved — single system author |
| Viewport REST endpoints | R-008 | Resolved — three new routes under `read` scope |
| Maintenance window | R-009 | Resolved — reuse `MaintenanceState` |
| Prometheus cardinality | R-010 | Resolved — name+model only |
| Tool naming | R-011 | Resolved — snake_case |
| Embeddings | R-012 | Resolved — server-side only |

No `NEEDS CLARIFICATION` markers remain in Technical Context.
