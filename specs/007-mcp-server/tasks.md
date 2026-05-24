---
description: "Task list for feature 007-mcp-server"
---

# Tasks: MCP Memory Server

**Input**: Design documents in [specs/007-mcp-server/](./)
**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md), [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md)

**Tests**: INCLUDED. The plan explicitly enumerates a `tests/integration/mcp_*.rs` suite plus a `tests/unit/klams-types/auth_scope.rs` unit test; each FR-NNN maps to one of those slots ([plan.md → Source Code](./plan.md#source-code-repository-root)).

**Organization**: One phase per user story (US1..US5) after shared Setup + Foundational phases. Each user-story phase is independently completable and independently testable per its **Independent Test** criterion in [spec.md](./spec.md).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: parallel-safe (touches different files, no in-flight dependency)
- **[Story]**: maps the task to a user story (US1..US5); omitted for shared phases
- Every task gives an exact file path

---

## Phase 1: Setup

**Purpose**: workspace scaffolding for the new MCP surface.

- [X] T001 SDK checkpoint: verify latest `rmcp` crate supports Streamable HTTP server transport on tokio 1.x + axum 0.7; record version pin and any caveats in [specs/007-mcp-server/research.md](./research.md#r-002--rust-mcp-sdk-selection). If unsupported, switch the plan's transport line to "hand-rolled per [MCP 2025-06-18 spec](https://modelcontextprotocol.io/specification/2025-06-18)" before T016.
- [X] T002 Create `crates/klams-mcp/Cargo.toml` and `crates/klams-mcp/src/lib.rs` skeleton (re-exports + `pub fn router(...) -> axum::Router` stub returning empty router); add `klams-mcp` to the workspace `members` in `/home/ken/src/ai/klams/Cargo.toml`.
- [X] T003 [P] Add `rmcp` (pinned per T001), `jsonschema = "0.18"`, and `subtle = "2"` (if not already top-level) to `crates/klams-mcp/Cargo.toml`; ensure crate inherits workspace lints (`unsafe_code = "forbid"`, clippy pedantic) and that `cargo check -p klams-mcp` passes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: schema, projection types, scoped auth, transport, and observability that every user story depends on.

**Critical**: No US-tagged task may start until this phase is green under `just gate`.

### Migrations & store changes

- [X] T004 SQL migration `migrations/0005_authors_table.sql` per [data-model.md §2](./data-model.md); includes seed `INSERT ... ON CONFLICT DO NOTHING` for `SYSTEM_AUTHOR_ID = 00000000-0000-7000-8000-000000000001`.
- [X] T005 SQL migration `migrations/0006_facts_author_and_soft_delete.sql` (adds `author_id` UUID NOT NULL with `DEFAULT '00000000-0000-7000-8000-000000000001'` for backfill, FK to `authors(id)`, `deleted_at`, `deleted_by_author_id`, and supporting indexes per [data-model.md §3](./data-model.md)).
- [X] T006 SQL migration `migrations/0007_events_author.sql` (adds `author_id` only — events stay append-only per [research.md R-006](./research.md#r-006--events-append-only-stance)).

### Projection & config types (klams-types)

- [X] T007 [P] Create `crates/klams-types/src/author.rs` with `pub struct Author`, `pub struct PublicAuthorRef`, and `pub const SYSTEM_AUTHOR_ID: uuid::Uuid` matching the migration's UUID; re-export from `crates/klams-types/src/lib.rs`.
- [X] T008 [P] Create `crates/klams-types/src/auth.rs` with `Scope` enum (`Read`/`Write`/`Admin`), `ScopeSet` bit-set helper, `TokenGrantConfig`, `TokenGrant`; extend `crates/klams-types/src/config.rs` so `AuthConfig` gains `tokens: Vec<TokenGrantConfig>` while keeping the existing `bearer_token: Option<String>` (FR-018 backward compatibility), per [data-model.md §5](./data-model.md).
- [X] T009 [P] Create `crates/klams-types/src/memory.rs` with `PublicMemory`, `MemoryKind` (`Fact`/`Knowledge`/`Event`), and `PublicMemoryContent` enum variants matching the per-kind shapes in [spec.md → Key Entities](./spec.md#key-entities) and [data-model.md §4](./data-model.md); re-export from lib.

### Storage layer (klams-store)

- [X] T010 Extend `crates/klams-store/src/postgres.rs` with author-store functions (`insert_author`, `get_author_by_id`, `list_authors_with_counts`, `touch_author_last_seen_at`) appended to the existing single-file module.
- [X] T011 Extend `crates/klams-store/src/postgres.rs` fact write path to require `author_id`; add `soft_delete_fact(id, by_author_id)`, `restore_fact(id)`, `hard_delete_fact(id)`, `list_deleted_facts(...)`; default fact reads filter `WHERE deleted_at IS NULL`.
- [X] T012 Extend `crates/klams-store/src/postgres.rs` event write path to require `author_id`; reads include a join on `authors` for projection.
- [X] T013 Extend `crates/klams-store/src/qdrant.rs` per [research.md R-003](./research.md#r-003--soft-delete-representation-in-qdrant): default search adds a `must_not` filter on `deleted_at`; add `soft_delete_payload`, `restore_payload`, `hard_delete_point` helpers; `memory_admin_list_deleted` queries via `must` on `deleted_at`.
- [X] T014 Create `crates/klams-store/src/backfill_qdrant_authors.rs` — idempotent one-shot that sets `author_id = SYSTEM_AUTHOR_ID` on any Qdrant point missing it; exposes `run_backfill(client, &CancellationToken)`.

### Scoped auth (klams-api)

- [X] T015 Refactor `crates/klams-api/src/auth.rs`: replace single-token comparison with `Arc<Vec<TokenGrant>>` and a constant-time loop per [research.md R-004](./research.md#r-004--multi-token-constant-time-comparison) (unconditional `ct_eq` across every grant, no early exit); add `require_scope(scope: Scope)` axum middleware extractor; extend `crates/klams-api/src/error.rs` with `ScopeInsufficient -> 403`.

### MCP crate plumbing (klams-mcp)

- [X] T016 Implement `crates/klams-mcp/src/lib.rs::router(state) -> axum::Router` returning a stub `/mcp` mount + `tools/list` returning an empty tool registry, using `rmcp` (or the fallback chosen at T001).
- [X] T017 Implement `crates/klams-mcp/src/transport.rs` exposing Streamable HTTP as primary and HTTP+SSE as a fallback on the same mount point per [research.md R-001](./research.md#r-001--transport-selection).
- [X] T018 Implement `crates/klams-mcp/src/auth_bridge.rs`: extract `Authorization: Bearer <token>` from the request, resolve to `ScopeSet` via the `Arc<Vec<TokenGrant>>` shared with klams-api; return MCP `UNAUTHORIZED` envelope when no match.
- [X] T019 Implement `crates/klams-mcp/src/errors.rs` with the constants from [contracts/error-codes.md](./contracts/error-codes.md) and a helper for the `{"isError": true, "content": [...], "_meta": {"error_code": "..."}}` envelope.
- [X] T020 Implement `crates/klams-mcp/src/projection.rs`: pure functions mapping internal `Fact`/`KnowledgeItem`/`Event` to `PublicMemory` — must drop `version`, `decay`, `confidence`, embedding vectors, `source` trust-tier, and any other internal field per [spec.md FR-011](./spec.md) and [data-model.md §4](./data-model.md).
- [X] T021 Implement `crates/klams-mcp/src/maintenance.rs`: wraps `MaintenanceState::is_active()` and returns the `MAINTENANCE_WINDOW_ACTIVE` envelope (`_meta.retry_after_seconds = 30`) for every write tool entry per [research.md R-009](./research.md#r-009--maintenance-window-integration).
- [X] T022 Implement `crates/klams-mcp/src/metrics.rs`: three Prometheus counters per [research.md R-010](./research.md#r-010--cardinality-discipline-for-prometheus-author-labels) and [spec.md FR-022](./spec.md): `klams_mcp_writes_total{agent_name, model, kind}` (kind ∈ {fact, knowledge, event}), `klams_mcp_deletes_total{agent_name, model, mode}` (mode ∈ {soft, restored, hard}), `klams_mcp_search_total{agent_name, model}`. `author_id` MUST NOT be a label; the search counter MUST NOT carry the request's `kinds` set as a label.
- [X] T023 Implement `crates/klams-mcp/src/tools/mod.rs`: registry that exposes `tools/list` filtered by the caller's `ScopeSet` (FR-020); `memory_admin_*` tools MUST be hidden from non-admin callers, not merely 403 on call.
- [X] T023b Instrument the `tools/mod.rs` dispatch with a `tracing::info_span!("mcp.tool", tool = %name, author_id = %author_id, agent_name = %agent_name, model = %model)` wrapping every tool call (FR-023). The span MUST be entered before scope re-validation so denied calls are still traced.

### Service wiring

- [X] T024 Update `crates/klams-service/src/main.rs`: mount `klams_mcp::router(state)` at `/mcp`; spawn the T014 backfill once at startup before the axum `serve()` call; register the T022 metrics with the existing Prometheus registry.

### Foundation-level tests (must fail before, pass after)

- [X] T025 [P] Unit test `tests/unit/klams-types/auth_scope.rs` covering `ScopeSet` union/intersect/contains semantics.
- [X] T026 [P] Integration test `tests/integration/mcp_scope_gating.rs` — read-only token sees only read tools; admin sees all; non-admin caller of `memory_admin_restore` gets `INSUFFICIENT_SCOPE` (FR-020); also covers mid-session scope downgrade: token rotated from `admin` to `write` no longer sees `memory_admin_*` in a fresh `tools/list` call (spec Edge Cases).
- [X] T027 [P] Integration test `tests/integration/auth_scoped_tokens.rs` — multi-token config dispatches by scope; legacy `[auth] bearer_token = "..."` still authenticates as full-scope (FR-017, FR-018, FR-019).
- [X] T028 [P] Integration test `tests/integration/mcp_maintenance_window.rs` — write tools return `MAINTENANCE_WINDOW_ACTIVE` while `MaintenanceState::is_active()` is true; reads continue to serve (FR-021).

**Checkpoint**: `just gate` green; `/mcp` returns empty `tools/list` over Streamable HTTP. User-story phases unblocked.

---

## Phase 3: User Story 1 — GHCP records a learned fact mid-session (P1) 🎯 MVP

**Goal**: An MCP client can `register_author` once per session then `memory_add` a fact or knowledge item attributed to that author.
**Independent Test**: run [quickstart.md steps 4–6](./quickstart.md); verify the new fact is visible in the viewport and carries the correct `author_id`.

### Tests for US1 (write FIRST, ensure they fail)

- [X] T029 [P] [US1] Integration test `tests/integration/mcp_register_author.rs` — `register_author` happy path returns a UUID v7; `touch_last_seen_at` updates on repeat call with same `agent_name` (FR-004, FR-006).
- [X] T030 [P] [US1] Integration test `tests/integration/mcp_memory_add_fact.rs` — fact persists with FK to `authors`, returns `PublicMemory` envelope, no internal fields leak (FR-009, FR-011); also covers `UNKNOWN_AUTHOR_ID`: calling `memory_add` with a UUID that does not exist in `authors` returns the error and writes nothing (spec Edge Cases).
- [X] T031 [P] [US1] Integration test `tests/integration/mcp_memory_add_knowledge.rs` — server computes embedding via TEI (no client vector accepted) per [research.md R-012](./research.md#r-012--embedding-policy-for-knowledge); soft-delete columns initialize to NULL; also covers `EMBEDDING_UNAVAILABLE`: when the TEI adapter errors, the tool returns the retryable error envelope with `retry_after_seconds` populated and no row is written (spec Edge Cases).

### Implementation for US1

- [X] T032 [US1] Implement `crates/klams-mcp/src/tools/register_author.rs` with input validation against `contracts/tool-schemas/register_author.json`; touches `authors.last_seen_at` on repeat.
- [X] T033 [US1] Implement `crates/klams-mcp/src/tools/memory_add.rs` dispatching on `kind` (`fact` | `knowledge`); knowledge path calls the existing TEI adapter; both increment `klams_mcp_writes_total{agent_name, model, kind}`.
- [X] T034 [US1] Wire `memory_add` `fact` path through the existing dedupe pipeline in `klams-core` so MCP-submitted facts deduplicate the same way as REST-submitted ones.

**Checkpoint**: An MCP client (VS Code or `just mcp-call`) can register and add facts/knowledge end-to-end.

---

## Phase 4: User Story 2 — Agent retrieves context before answering (P1)

**Goal**: An MCP client can semantically search memory and pull related items.
**Independent Test**: [quickstart.md step 7](./quickstart.md); `memory_search` returns < 1 s p95 at fixture scale (SC-006); soft-deleted items are excluded.

### Tests for US2

- [X] T035 [P] [US2] Integration test `tests/integration/mcp_memory_search.rs` — projection scrubs internal fields (FR-011); soft-deleted items excluded by default; tag filter honored.
- [X] T036 [P] [US2] Integration test `tests/integration/mcp_memory_related.rs` — related lookup honors the same soft-delete filter and returns the configured `top_k`.

### Implementation for US2

- [X] T037 [US2] Implement `crates/klams-mcp/src/tools/memory_search.rs` (Postgres FTS for facts, Qdrant for knowledge, in-memory merge by score per [research.md R-008](./research.md#r-008--rest-endpoints-for-viewport)); applies the soft-delete filter; increments `klams_mcp_search_total`.
- [X] T038 [US2] Implement `crates/klams-mcp/src/tools/memory_related.rs` (Qdrant nearest-neighbor on the originating point's vector).

**Checkpoint**: Search + related work end-to-end and satisfy SC-006 at the fixture dataset size.

---

## Phase 5: User Story 3 — Agent records a deployment event (P2)

**Goal**: An MCP client can append a typed, immutable event.
**Independent Test**: [quickstart.md step 8](./quickstart.md); event appears in `events` table with correct `author_id` and is not deletable via `memory_delete`.

### Tests for US3

- [X] T039 [P] [US3] Integration test `tests/integration/mcp_memory_append_event.rs` — round-trip; `category` + `payload` persisted; attempting `memory_delete` on the returned id returns `EVENTS_NOT_DELETABLE` (FR-015).

### Implementation for US3

- [X] T040 [US3] Implement `crates/klams-mcp/src/tools/memory_append_event.rs` validating against `contracts/tool-schemas/memory_append_event.json`; rejects with `MAINTENANCE_WINDOW_ACTIVE` during the backup window.

**Checkpoint**: Events flow end-to-end and remain append-only.

---

## Phase 6: User Story 4 — Agent makes a mistake and the system recovers (P2)

**Goal**: A misbehaving agent's writes can be soft-deleted by any write-scoped caller, restored or hard-deleted by an admin, and listed for review.
**Independent Test**: [quickstart.md steps 9–10 (rogue-agent drill)](./quickstart.md); soft-deleted items disappear from `memory_search`, reappear after `memory_admin_restore`.

### Tests for US4

- [X] T041 [P] [US4] Integration test `tests/integration/mcp_memory_delete_soft.rs` — soft delete sets `deleted_at` + `deleted_by_author_id`, is idempotent (FR-014), and events are not deletable (FR-015).
- [X] T042 [P] [US4] Integration test `tests/integration/mcp_admin_restore.rs` — restore clears soft-delete columns and the item reappears in `memory_search`.
- [X] T043 [P] [US4] Integration test `tests/integration/mcp_admin_hard_delete.rs` — hard delete removes Postgres row + Qdrant point; subsequent restore returns `NOT_FOUND`.
- [X] T044 [P] [US4] Integration test `tests/integration/mcp_admin_list_deleted.rs` — cursor pagination over the `deleted_at IS NOT NULL` slice; filter by `author_id` and `since`.
- [X] T045 [P] [US4] Integration test `tests/integration/mcp_rogue_agent_drill.rs` — the full SC-008 drill: register rogue author → spam writes → admin soft-deletes them → search no longer surfaces them → hard-delete cleans up.

### Implementation for US4

- [X] T046 [US4] Implement `crates/klams-mcp/src/tools/memory_delete.rs` (soft only; rejects event ids with `EVENTS_NOT_DELETABLE`); increments `klams_mcp_deletes_total{agent_name, model, mode = "soft"}`.
- [X] T047 [US4] Implement `crates/klams-mcp/src/tools/memory_admin_restore.rs`; increments `klams_mcp_deletes_total{agent_name, model, mode = "restored"}`.
- [X] T048 [US4] Implement `crates/klams-mcp/src/tools/memory_admin_hard_delete.rs` (Postgres `DELETE` + Qdrant `delete_points`); increments `klams_mcp_deletes_total{agent_name, model, mode = "hard"}`.
- [X] T049 [US4] Implement `crates/klams-mcp/src/tools/memory_admin_list_deleted.rs` with opaque cursor pagination.

**Checkpoint**: The rogue-agent drill succeeds end-to-end (SC-008).

---

## Phase 7: User Story 5 — Ken reviews per-author activity (P3)

**Goal**: The viewport's new `/authors` route lists authors with write counts and lets Ken drill into per-author memories — via REST, not MCP.
**Independent Test**: [quickstart.md step 11](./quickstart.md); `/authors` lists all registered agents with counts; clicking one shows their memories.

### Tests for US5

- [ ] T050 [P] [US5] Integration test `tests/integration/api_authors_list.rs` — `GET /v1/authors?limit=50` returns all authors with `last_seen_at` and the `counts` object `{writes, soft_deletes, restores_received, events}` per [contracts/rest-authors.md](./contracts/rest-authors.md) (FR-024 + FR-024a).
- [ ] T051 [P] [US5] Integration test `tests/integration/api_authors_detail.rs` — `GET /v1/authors/{id}` returns full `Author` projection.
- [ ] T052 [P] [US5] Integration test `tests/integration/api_authors_memories.rs` — `GET /v1/authors/{id}/memories?kinds=fact,knowledge,event` returns `PublicMemory` rows paginated.

### Implementation for US5

- [ ] T053 [US5] Implement `crates/klams-api/src/handlers/authors.rs` (three handlers) and mount in `crates/klams-api/src/router.rs` behind `require_scope(Scope::Read)` per [contracts/rest-authors.md](./contracts/rest-authors.md).
- [ ] T054 [P] [US5] viewport route `viewport/src/routes/authors/+page.svelte` + `+page.ts` (list view calling `GET /v1/authors`).
- [ ] T055 [P] [US5] viewport route `viewport/src/routes/authors/[id]/+page.svelte` + `[id]/+page.ts` (detail view calling `GET /v1/authors/{id}` and `.../memories`). Each memory row MUST render a state badge (`live` | `soft-deleted` | `hard-deleted`) and link `{id, kind}` to the matching detail in `/facts/{id}`, `/knowledge/{id}`, or `/events/{id}` so Ken can follow the row into the existing per-kind routes (FR-025).

**Checkpoint**: All user stories independently functional.

---

## Phase 8: Polish & Cross-Cutting

- [ ] T056 [P] Update `docs/architecture.md` with a new §2e "MCP projection layer" covering the authors table, the public projection, the scope-gated tool surface, and soft-delete representation.
- [ ] T057 [P] Update `docs/setup.md` with MCP registration sections for both VS Code (`.vscode/mcp.json`) and the GHCP CLI (`~/.copilot/mcp-config.json`); copy the working examples from [quickstart.md](./quickstart.md).
- [ ] T058 [P] Update `docs/usage.md` with the MCP chapter: tool surface, scope configuration, soft-delete safety model, and the viewport `/authors` review workflow.
- [ ] T059 [P] Add a commented `[[auth.tokens]]` block to `deploy/config/klams.example.toml` mirroring [data-model.md §5](./data-model.md).
- [ ] T060 [P] Add `mcp-call` recipe to the root `justfile` (used by [quickstart.md](./quickstart.md) and ops scripts).
- [ ] T061 Execute all 12 steps of [quickstart.md](./quickstart.md) against a fresh test instance; record observed timings against SC-001..SC-008.
- [ ] T062 Validate SC-006 explicitly: load fixture with ≥ 10k facts + 50k knowledge items, run `memory_search` 100× and record p95. Attach the result to the PR description. **Do not** start tuning work if the p95 exceeds 1 s — surface the measurement to the user first and let them decide whether the actual overshoot is "good enough" for the homelab before any optimization work begins (per SC-006 note).
- [ ] T063 Final `just gate` pass; resolve any clippy/fmt drift introduced during integration.

---

## Dependencies & Execution Order

### Phase dependencies

- Phase 1 → Phase 2 → (Phase 3 ∥ Phase 4 ∥ Phase 5 ∥ Phase 6 ∥ Phase 7) → Phase 8
- All `[P]` tasks inside a phase can run in parallel.
- All five user-story phases can run in parallel after Phase 2 completes (single-developer reality: sequential P1 → P2 → P3 still yields incremental value).

### Intra-phase dependencies

- T002 blocks T003, T016, T024.
- T004–T006 block T010–T014 (store needs schema).
- T007–T009 block T010–T024 and every tool implementation.
- T015 blocks T018, T024, T026, T027.
- T016–T023 block T024 and every `tools/*.rs` task.
- T023 blocks T023b (tracing instrumentation wraps the dispatch).
- T023b blocks every `tools/*.rs` task (T032, T033, T037, T038, T040, T046, T047, T048, T049) so every tool call inherits the span.
- T010, T011 block T029–T034 (US1 writes).
- T013 blocks T035–T038 (US2 search/related).
- T012 blocks T039, T040 (US3 events).
- T011, T013 block T041–T049 (US4 soft-delete + admin).
- T010 + T015 block T050–T053 (US5 REST).
- T053 blocks T054, T055 (viewport needs REST to call).
- T061 blocks T062 only on the same fixture instance; otherwise independent.

---

## Parallel example — Phase 2 foundational types

```bash
# Once T002 and migrations land, three klams-types tasks can run together:
Task: T007 — create crates/klams-types/src/author.rs (Author, PublicAuthorRef, SYSTEM_AUTHOR_ID)
Task: T008 — create crates/klams-types/src/auth.rs (Scope, TokenGrant, AuthConfig.tokens)
Task: T009 — create crates/klams-types/src/memory.rs (PublicMemory, MemoryKind, PublicMemoryContent)
```

## Parallel example — Phase 6 US4 test suite

```bash
# Five integration tests can be authored in parallel (all touch separate files):
Task: T041 — tests/integration/mcp_memory_delete_soft.rs
Task: T042 — tests/integration/mcp_admin_restore.rs
Task: T043 — tests/integration/mcp_admin_hard_delete.rs
Task: T044 — tests/integration/mcp_admin_list_deleted.rs
Task: T045 — tests/integration/mcp_rogue_agent_drill.rs
```

---

## Implementation Strategy

### MVP slice (recommended first deploy)

1. Phase 1 + Phase 2 → foundation green under `just gate`.
2. Phase 3 (US1) → register + write end-to-end.
3. Phase 4 (US2) → read end-to-end.
4. **STOP and validate** — register VS Code's MCP client and run the quickstart's first six steps live. This is enough for daily Copilot use to start producing klams memories.

### Incremental delivery thereafter

- Phase 5 (US3 events) adds deployment provenance.
- Phase 6 (US4 soft-delete + admin) adds the rogue-agent safety story; this is the **gate for opening MCP to homelab agents beyond Copilot**.
- Phase 7 (US5 viewport `/authors`) gives Ken the review surface.
- Phase 8 polishes docs and validates SCs.

### Single-developer notes

- All user-story phases are independent at the schema level — incomplete US4 admin tools do not break US1/US2/US3.
- The soft-delete columns added in Phase 2 (T005) make Phase 6's tools cheap to add later; resist the temptation to defer the columns.

---

## Notes

- `[P]` = different files, no in-flight dependency. Sequential tasks within a phase usually share a file (e.g., several tool registrations all edit `tools/mod.rs`).
- `[Story]` is omitted on Setup/Foundational/Polish tasks by design.
- Verify tests fail before implementing.
- Commit per task or per logical group; the after_implement hook offers an auto-commit.
- The rogue-agent drill (T045) is the headline acceptance test — passing it is the practical signal that the soft-delete safety story holds.
