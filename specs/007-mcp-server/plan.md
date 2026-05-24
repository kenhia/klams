# Implementation Plan: MCP Memory Server

**Branch**: `007-mcp-server` | **Date**: 2026-05-24 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/007-mcp-server/spec.md`

## Summary

Add a Model Context Protocol (MCP) server to `klams-service` so that external agents — GitHub Copilot in VS Code and the GHCP CLI first, other homelab agents later — can read and write klams memory through a small, projection-only public surface. The server mounts on the existing axum router, shares the existing tokio runtime and configuration loader, and is gated by an extended bearer-token auth that now carries per-token scopes (`read`, `write`, `admin`).

Every memory row written via MCP carries a server-issued `author_id` referencing a new `authors` table, captured once per session via a `register_author` tool. The public `Memory` projection drops internal bookkeeping (decay, confidence, raw embedding vectors, optimistic-concurrency `version`, internal trust-tier `source`) and exposes a single `kind`-discriminated envelope across facts, knowledge, and events. `memory_delete` is **soft** for everyone; only `admin`-scoped tokens see `memory_admin_restore`, `memory_admin_hard_delete`, and `memory_admin_list_deleted` in `tools/list`, and only those tokens can call them.

The viewport's `/authors` route reads from three **new REST endpoints** on `klams-service` (not MCP), keeping the desktop app on the REST contract it already uses. Per-author analytics land in Prometheus with low-cardinality `agent_name` + `model` labels (never `author_id`); per-row drilldown is the viewport's job.

Two cross-sprint integrations are reused as-is: sprint 006's `MaintenanceState` gates MCP write tools just as it gates REST writes, and the existing TEI adapter computes embeddings for `memory_add` of `kind = "knowledge"` — clients never supply vectors.

Three additional planning-time tweaks vs. the spec sketch:

1. **Soft delete in Qdrant uses payload fields**, not a separate collection (see [research.md R-003](./research.md#r-003--soft-delete-representation-in-qdrant)). Zero schema migration, one extra filter clause on the default search.
2. **`rmcp` is the target SDK**, with a research checkpoint at T001 to confirm Streamable-HTTP server support before any production code lands ([research.md R-002](./research.md#r-002--rust-mcp-sdk-selection)).
3. **`author_id` is not bound to a bearer token** ([research.md R-005](./research.md#r-005--author-identity-binding-to-bearer-tokens)) — any `write`-scoped token can submit on behalf of any registered author. Single-user system, no tenant boundary.

## Technical Context

**Language/Version**: Rust 1.94.1 stable (`rust-toolchain.toml`), edition 2021 (workspace pinned in `Cargo.toml`)  
**Primary Dependencies**: existing — `tokio` 1.x, `axum` 0.7, `sqlx` 0.8 (Postgres), `qdrant-client` 1.12, `reqwest` 0.12, `tracing` 0.1, `prometheus` 0.13, `serde` 1, `serde_json` 1, `chrono` 0.4 (with `serde`), `uuid` 1 (with `v7`), `subtle` 2 (constant-time compare), `jsonschema` 0.18; new — `rmcp` (latest 0.x compatible with axum 0.7 / tokio 1.x; pin at task-start)  
**Storage**: Postgres 16 (`authors` table; additive columns on `facts` + `events`), Qdrant 1.12.4 (payload-field soft-delete extension on `knowledge_items`), TEI HTTP embedder (unchanged)  
**Testing**: `cargo test --workspace` (unit + integration), `tests/docker-compose.test.yml` fixture, `just gate` mirroring CI (`cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace`), one new MCP integration suite hitting a live `klams-mcp` endpoint over Streamable HTTP  
**Target Platform**: Linux server (kubs0 systemd unit) for `klams-service`; viewport gains an `/authors` route built on the existing Tauri 2 + SvelteKit stack and continues to ship via `just viewport-build` (`x86_64-pc-windows-msvc` via `cargo-xwin`) plus native Linux for WSLg  
**Project Type**: Cargo workspace + Tauri/SvelteKit app. New crate `klams-mcp` joins the existing six service crates and is wired by `klams-service`. Additive changes to `klams-types` (config + projection + system author constant), `klams-store` (author repo, soft-delete columns, Qdrant payload filter), `klams-api` (scoped auth, `/v1/authors*` routes), and `klams-service` (MCP mount, startup backfill, Prometheus registration).  
**Performance Goals**: `memory_search` < 1 s p95 at homelab scale (≤ 10k facts, ≤ 50k knowledge items — SC-006); `tools/list` < 50 ms p95; `register_author` < 20 ms p95; viewport `/v1/authors?limit=50` < 50 ms p95  
**Constraints**: zero increase in unsafe code (workspace remains `unsafe_code = "forbid"`); constant-time token comparison preserved (R-004); Prometheus label cardinality bounded — every MCP counter is labeled by `agent_name` + `model`, `writes` adds `kind`, `deletes` adds `mode`; `author_id` MUST NOT appear as a label and the search counter MUST NOT carry the request's `kinds` set as a label (R-010); soft-delete columns add ≤ 2 columns per affected table; existing single-token `[auth] bearer_token = "..."` config remains valid (FR-018); MCP write tools MUST honor `MaintenanceState` (FR-021); events remain append-only (FR-015, R-006)  
**Scale/Scope**: single klams instance; O(100) author rows over the system's lifetime; O(10) bearer tokens in production config; one MCP mount serving multiple concurrent agents over Streamable HTTP (and HTTP+SSE as fallback for legacy clients)  

## Constitution Check

*Re-checked after Phase 1 design — both passes recorded below.*

| Principle | Initial gate (pre-design) | Post-Phase-1 gate | Notes |
|-----------|---------------------------|-------------------|-------|
| I. SDD | PASS | PASS | `spec.md` precedes any code; this plan + research/data-model/quickstart/contracts complete the SDD artifact set before tasks are emitted |
| II. TDD | PASS | PASS | Each FR-NNN maps to a contract or integration test slot listed under **Source Code** below; tool input schemas in `contracts/tool-schemas/` are testable independently of any handler; MCP error-code constants drive a contract test before any handler change |
| III. Code Standards | PASS | PASS | `just gate` is the unchanged exit gate; no new lints relaxed; no new clippy allows; one new crate added to the workspace lint set |
| IV. Documentation | PASS | PASS | `docs/setup.md` (MCP registration for both VS Code + GHCP CLI), `docs/usage.md` (tool surface + scope config + soft-delete safety model), `docs/architecture.md` (§2e MCP projection block + authors table + scope-gated tool surface), README link to viewport `/authors` page — all on the deliverables list (Phase 1 → Documentation) |
| V. Quality & Observability | PASS | PASS | Three new Prometheus counters with bounded label cardinality + structured `tracing` spans per MCP call (token-hash, `author_id`, `agent_name`, `model`, tool name); `/healthz` extension reports `mcp.enabled` and active transports; error codes are stable contract surface ([contracts/error-codes.md](./contracts/error-codes.md)) |
| VI. Simplicity & Intentional Design | PASS | PASS | Reuse over invention: existing axum mount, existing `MaintenanceState`, existing TEI embedder, existing constant-time auth (extended, not replaced), existing dedupe pipeline. Rate limits deferred; viewport stays REST-only; no separate admin binary; one MCP SDK; Qdrant soft-delete is two payload fields (not a new collection); author/token binding deliberately not added (R-005). Per-author analytics drilldown via REST instead of high-cardinality metric labels (R-010). |

No principle violations require justification in **Complexity Tracking**.

## Project Structure

### Documentation (this feature)

```text
specs/007-mcp-server/
├── spec.md                      # /speckit.specify output (with Clarifications section)
├── plan.md                      # this file
├── research.md                  # Phase 0 output (this run) — R-001..R-012
├── data-model.md                # Phase 1 output (this run) — tables, projection, migrations
├── quickstart.md                # Phase 1 output (this run) — 12-step walkthrough = acceptance script
├── contracts/                   # Phase 1 output (this run)
│   ├── README.md
│   ├── tools.md                 # Tool reference + scopes + output shapes
│   ├── rest-authors.md          # GET /v1/authors[/{id}[/memories]] contracts
│   ├── error-codes.md           # Canonical _meta.error_code values
│   └── tool-schemas/            # JSON Schema 2020-12 per tool input
│       ├── register_author.json
│       ├── memory_add.json
│       ├── memory_append_event.json
│       ├── memory_search.json
│       ├── memory_related.json
│       ├── memory_delete.json
│       ├── memory_admin_restore.json
│       ├── memory_admin_hard_delete.json
│       └── memory_admin_list_deleted.json
├── checklists/
│   └── requirements.md          # spec-quality checklist (from /speckit.specify)
└── tasks.md                     # Phase 2 output (NOT created here — /speckit.tasks)
```

### Source Code (repository root)

```text
crates/
├── klams-types/
│   ├── src/auth.rs              # NEW: Scope, ScopeSet, TokenGrant, TokenGrantConfig
│   ├── src/config.rs            # +AuthConfig.tokens (Vec<TokenGrantConfig>); preserve bearer_token
│   ├── src/author.rs            # NEW: Author, PublicAuthorRef, SYSTEM_AUTHOR_ID const
│   ├── src/memory.rs            # NEW: PublicMemory, MemoryKind, PublicMemoryContent
│   └── src/lib.rs               # re-exports
├── klams-store/
│   ├── src/postgres.rs          # +author store fns; +soft-delete helpers; +author_id on facts/events write paths
│   ├── src/qdrant.rs            # +payload soft-delete filter on default reads; restore/hard-delete
│   └── src/backfill_qdrant_authors.rs   # NEW: one-shot startup backfill (idempotent)
migrations/
├── 0005_authors_table.sql
├── 0006_facts_author_and_soft_delete.sql
└── 0007_events_author.sql
├── klams-api/
│   ├── src/auth.rs              # AuthState: Vec<TokenGrant>; require_scope middleware
│   ├── src/handlers/authors.rs  # NEW: GET /v1/authors[, /{id}[, /memories]]
│   ├── src/router.rs            # mount /v1/authors* under read scope
│   └── src/error.rs             # +ScopeInsufficient; map to 403
├── klams-mcp/                   # NEW CRATE
│   ├── Cargo.toml
│   ├── src/lib.rs               # facade + mount(router, state) -> Router
│   ├── src/transport.rs         # Streamable HTTP primary; SSE fallback on the same mount
│   ├── src/auth_bridge.rs       # extract token → ScopeSet from request; gate tools/list + tools/call
│   ├── src/projection.rs        # internal Fact/Event/KnowledgeItem -> PublicMemory
│   ├── src/maintenance.rs       # MAINTENANCE_WINDOW_ACTIVE envelope helper
│   ├── src/errors.rs            # MCP error envelope + error_code constants
│   ├── src/metrics.rs           # klams_mcp_writes_total, _deletes_total, _search_total
│   └── src/tools/
│       ├── mod.rs               # registry; tools_list filtered by ScopeSet
│       ├── register_author.rs
│       ├── memory_add.rs
│       ├── memory_append_event.rs
│       ├── memory_search.rs
│       ├── memory_related.rs
│       ├── memory_delete.rs
│       ├── memory_admin_restore.rs
│       ├── memory_admin_hard_delete.rs
│       └── memory_admin_list_deleted.rs
├── klams-service/
│   └── src/main.rs              # mount klams_mcp::router(...) under /mcp; register MCP metrics;
│                                # drive Qdrant author backfill at startup
└── (other crates unchanged)

viewport/
└── src/routes/authors/
    ├── +page.svelte             # list view (calls GET /v1/authors)
    ├── +page.ts                 # SvelteKit loader
    ├── [id]/+page.svelte        # detail view (calls GET /v1/authors/{id} and /memories)
    └── [id]/+page.ts

tests/
├── unit/
│   └── klams-types/auth_scope.rs            # ScopeSet bit-set semantics
├── integration/
│   ├── mcp_register_author.rs               # tool contract test (FR-004, FR-006)
│   ├── mcp_memory_add_fact.rs               # FR-009 fact path, author FK persisted
│   ├── mcp_memory_add_knowledge.rs          # FR-009 knowledge path, server-side embedding
│   ├── mcp_memory_append_event.rs           # FR-010
│   ├── mcp_memory_search.rs                 # FR-008 + FR-011 — projection scrubs internal fields
│   ├── mcp_memory_related.rs                # FR-012
│   ├── mcp_memory_delete_soft.rs            # FR-013, FR-014 (idempotency), FR-015 (events forbidden)
│   ├── mcp_admin_restore.rs                 # FR-016 restore round-trip
│   ├── mcp_admin_hard_delete.rs             # FR-016 hard-delete, NOT_FOUND on retry
│   ├── mcp_admin_list_deleted.rs            # FR-016 listing + pagination
│   ├── mcp_scope_gating.rs                  # FR-020 tools/list + tools/call gating
│   ├── mcp_maintenance_window.rs            # FR-021 503-equivalent envelope
│   ├── mcp_rogue_agent_drill.rs             # SC-008 end-to-end safety drill
│   ├── api_authors_list.rs                  # FR-024a GET /v1/authors
│   ├── api_authors_detail.rs                # FR-024a GET /v1/authors/{id}
│   ├── api_authors_memories.rs              # FR-024a GET /v1/authors/{id}/memories
│   └── auth_scoped_tokens.rs                # FR-017, FR-018 (legacy form still works), FR-019
└── fixtures/
    └── mcp/
        ├── authors.sql                       # seed multiple authors for tests
        └── mixed_facts_knowledge.sql         # cross-kind dataset for search/related

docs/
├── architecture.md              # +§2e MCP projection layer + authors table + scope-gated surface
├── setup.md                     # +MCP registration: .vscode/mcp.json and ~/.copilot/mcp-config.json
└── usage.md                     # +MCP chapter: tools, scopes, soft-delete safety, viewport /authors

justfile
└── + mcp-call <tool> <json>     # convenience recipe used by quickstart.md and ops scripts

deploy/config/klams.example.toml
└── +commented [[auth.tokens]] block matching data-model.md §5
```

**Structure Decision**: a new `klams-mcp` crate. Justification: the MCP surface is its own integration boundary with its own SDK dependency (`rmcp`); keeping it in a separate crate (a) keeps `klams-api` free of MCP SDK types, (b) lets the `tools/` module mirror the contract files 1:1 for review-time correspondence, and (c) makes it trivial to disable MCP at the `klams-service` mount site if a deployment needs the REST API only. The crate is wired by `klams-service` and depends downward on `klams-types`, `klams-store`, and `klams-core`, identical to how `klams-api` depends — no new graph shape.

## Complexity Tracking

> No Constitution Check violations require justification.

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|--------------------------------------|
| _(none)_  | _(none)_   | _(none)_                             |
