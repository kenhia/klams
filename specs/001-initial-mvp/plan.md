# Implementation Plan: klams Initial MVP

**Branch**: `001-initial-mvp` | **Date**: 2026-05-16 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-initial-mvp/spec.md`

## Summary

Deliver the first shippable slice of klams: a Rust-based async memory
service running on `kubs0` that exposes HTTP/JSON endpoints for facts,
events, and knowledge items; backs facts/events with PostgreSQL and
knowledge with Qdrant (with GPU-backed embeddings); and a companion
Tauri 2 + Svelte 5 desktop **viewport** built as a single Windows
binary that reads and inspects all three memory surfaces.

Stateful dependencies (PostgreSQL, Qdrant, the embedding inference
server) ship as a single `docker compose` stack on `kubs0`. The klams
service itself runs as a systemd-managed binary against that stack —
the decision rationale is in [research.md](research.md) §3. The
viewport is cross-built from Linux via `cargo-xwin` and ships as
`klams-viewport.exe` without an installer.

## Technical Context

**Language/Version**:

- Service & shared crates: Rust (stable, MSRV pinned at scaffold time
  via `rust-toolchain.toml`).
- Viewport backend: Rust (same toolchain), Tauri 2.x.
- Viewport frontend: TypeScript + Svelte 5 (SvelteKit static adapter)
  built with Vite.

**Primary Dependencies**:

- Service: `tokio`, `axum` (HTTP), `tower`/`tower-http`, `serde`,
  `sqlx` (Postgres, compile-time-checked queries), `qdrant-client`
  (Qdrant gRPC), `reqwest` (embedding HTTP calls), `tracing` +
  `tracing-subscriber`, `axum-prometheus`, `uuid`, `time`,
  `thiserror`, `subtle`.
- Embeddings: Hugging Face Text Embeddings Inference (TEI) running as
  a Docker container with GPU access; klams calls it over HTTP. See
  [research.md](research.md) §1.
- Viewport: `tauri` 2.x, `tauri-plugin-store`, `keyring`, `reqwest`,
  SvelteKit + `@sveltejs/adapter-static`, TypeScript.
- Shared: `klams-types` (DTOs via `serde`); `klams-client` (typed
  HTTP wrapper used by controller + viewport backend).

**Storage**:

- PostgreSQL 16 (Compose service, dedicated DB `klams`, dedicated
  role `klams`, bind-mounted data dir under `${KLAMS_DATA_ROOT}/postgres`).
- Qdrant 1.x (Compose service, on-disk persistence, single node,
  gRPC client from klams; data under `${KLAMS_DATA_ROOT}/qdrant`).
- Embedding model files cached under `${KLAMS_DATA_ROOT}/tei`.
- **Storage root**: a single configurable path (`KLAMS_DATA_ROOT`,
  default `/ai/klams/data`) holds every stateful service volume.
  The klams service's own state — config, sqlx offline data, optional
  log spool — lives under `KLAMS_ROOT` (default `/ai/klams`).
  Layout, env vars, and rationale: [research.md §12](research.md#12-storage-root).

**Testing**:

- `cargo test` for unit tests (per-crate).
- Integration tests under `crates/klams-service/tests/` run against an
  ephemeral Compose stack defined in `tests/docker-compose.test.yml`
  (Postgres + Qdrant + TEI on a private network with ephemeral
  volumes).
- Viewport frontend: `vitest` for `lib/api.ts` wrappers only in MVP.
- Viewport backend: `cargo test` for Tauri command wrappers with a
  trait-mocked `klams-client`.

**Target Platform**:

- Service: Linux x86_64 (`kubs0`), systemd-managed binary.
- Viewport: Windows 10/11 x86_64, single executable
  `klams-viewport.exe`. Cross-built on Linux via `cargo-xwin`
  targeting `x86_64-pc-windows-msvc`.

**Project Type**: Multi-component workspace — backend service + desktop
GUI client. Maps to a Cargo workspace at the repo root plus a sibling
`viewport/` directory containing its own Cargo workspace (Tauri
convention) and a Vite/SvelteKit frontend.

**Performance Goals** (from spec SC-001..009):

- Fact write + retrieve via search round-trip < 2 s on LAN.
- Knowledge chunk searchable within 10 s at p95 under MVP load
  (≤ 100 indexing req/min).
- Unified search p95 < 500 ms at the MVP corpus size
  (10k facts / 50k events / 10k knowledge items).
- Viewport cold launch → populated dashboard in < 3 s.

**Constraints**:

- LAN-only deployment; bearer token over plain HTTP is acceptable.
- Single-host deployment on `kubs0`; no horizontal scaling.
- GPU is shared with other workloads; TEI must release VRAM when idle.
- Every successful write survives a service restart (SC-004).
- **All Compose services attach to a single user-defined bridge
  network `klams-net`**; the default `bridge` network is forbidden.
  Services use deterministic DNS aliases (`postgres`, `qdrant`,
  `tei`, future `redis`/`grafana`). Rationale and pattern:
  [research.md §13](research.md#13-docker-network).
- **All persistent storage lives under `KLAMS_DATA_ROOT`** (default
  `/ai/klams/data`); no Docker named volumes for stateful services.
  Rationale: [research.md §12](research.md#12-storage-root).

**Scale/Scope** (MVP target):

- ~10k facts, ~50k events, ~10k knowledge items.
- One concurrent writer (the controller) plus the viewport as a
  reader; ≤ 5 concurrent in-flight requests typical.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Verdict | Notes |
|---|---|---|
| I. Spec-Driven Development | PASS | Plan derives entirely from [spec.md](spec.md); no out-of-spec features added. |
| II. Test-Driven Development | PASS | Per-crate unit tests + integration tests against a Compose stack; quickstart enforces test-first. |
| III. Code Standards Gate | PASS | CI commands match constitution §"Pre-Commit Checks" (see [quickstart.md](quickstart.md)). |
| IV. Documentation | PASS | Phase 1 outputs include `quickstart.md`; FR-030 explicitly requires `docs/architecture.md`, `docs/setup.md`, `docs/usage.md` updates. |
| V. Quality & Observability | PASS | Structured `tracing` logs, Prometheus `/metrics`, `/healthz`, actionable errors. FR-017..020. |
| VI. Simplicity & Intentional Design | PASS | YAGNI honored: no gRPC API, no TLS, no multi-tenant, no MCP, no decay yet. One embedding backend. One DB, one vector store, one binary deployment. |

**Re-check after Phase 1 design**: PASS — see note at end of Phase 1.
No violations to track in the Complexity Tracking section.

## Project Structure

### Documentation (this feature)

```text
specs/001-initial-mvp/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   ├── openapi.yaml     # klams HTTP API contract
│   └── viewport-commands.md  # Tauri command contract
├── checklists/
│   └── requirements.md  # Spec quality checklist
└── tasks.md             # Phase 2 output (NOT created by /speckit.plan)
```

### Source Code (repository root)

```text
Cargo.toml                  # workspace root
rust-toolchain.toml
crates/
  klams-types/              # shared DTOs: Fact, Event, KnowledgeItem, MemoryWrite, SearchResult, HealthSnapshot
  klams-core/               # bounded mpsc queue, worker pool, MemoryWrite dispatch, dedupe hashing
  klams-store/              # Postgres (sqlx) + Qdrant (qdrant-client) + TEI embedding adapters
  klams-api/                # axum router, auth middleware, request validation, error mapping
  klams-service/            # binary: wires everything; loads config, starts queue + workers + HTTP server
  klams-client/             # typed HTTP client crate (used by controller + viewport backend)
tests/
  docker-compose.test.yml   # ephemeral Postgres + Qdrant + TEI for `cargo test`
  fixtures/
migrations/                 # sqlx-managed SQL migrations for facts, events
deploy/
  docker-compose.yml        # production stack on kubs0 (postgres, qdrant, tei; klams optional/commented)
  systemd/
    klams.service           # systemd unit
  config/
    klams.example.toml      # example service config (bearer token, URLs, queue sizing)
docs/
  architecture.md
  setup.md
  usage.md
viewport/
  Cargo.toml                # viewport workspace root
  rust-toolchain.toml
  src-tauri/
    Cargo.toml
    tauri.conf.json
    build.rs
    src/
      main.rs
      config.rs             # %APPDATA%/klams/viewport.toml + keyring
      commands/
        memory.rs           # tauri::command wrappers around klams-client
        health.rs
  src/                      # Svelte 5 + SvelteKit (static adapter)
    routes/
      +layout.svelte
      +page.svelte          # dashboard: URL, version, health
      facts/+page.svelte
      events/+page.svelte
      knowledge/+page.svelte
    lib/
      api.ts                # invoke() wrappers
      types.ts              # mirrors klams-types DTOs
      stores.ts             # connection status, last refresh
  static/
  svelte.config.js
  vite.config.ts
  package.json
  pnpm-lock.yaml
  README.md
  xwin/                     # cargo-xwin SDK cache (gitignored)
```

**Structure Decision**:
A single Git repository with two co-located but independent Cargo
workspaces:

1. **Repo-root workspace** — all service and shared crates (including
   `klams-client`). Builds and runs on `kubs0`.
2. **`viewport/` workspace** — Tauri 2 backend (`src-tauri/`)
   depending on `klams-client` via a `path = "../../crates/klams-client"`
   dependency, plus the SvelteKit frontend rooted at `viewport/src/`.

Rationale: Tauri's tooling expects `tauri.conf.json` adjacent to a
single `src-tauri` Cargo project; mixing it into the main workspace
fights `pnpm tauri build`. Sharing `klams-client` via a path
dependency keeps the Windows binary in sync with the service contract
without duplicating DTOs.

Stateful services (PostgreSQL, Qdrant, TEI) ship as a single
[`deploy/docker-compose.yml`](../../deploy/docker-compose.yml) on
`kubs0`. The klams service itself runs via systemd in the MVP (see
[research.md](research.md) §3); a commented-out Compose service entry
for `klams` is included for developer convenience.

## Complexity Tracking

No constitution violations to justify.

## Phase 0 — Research

See [research.md](research.md). All items the spec deferred to plan
phase are resolved there:

| Decision | Outcome (summary) |
|---|---|
| Embedding model + runtime | HF Text Embeddings Inference (TEI) in Docker w/ `BAAI/bge-small-en-v1.5`, 384-dim, cosine. |
| Postgres deployment mode | Dedicated Compose-managed Postgres 16; dedicated `klams` DB + role. |
| klams service deployment | systemd-managed binary on `kubs0`; dependencies via Compose. |
| Qdrant deployment | Compose service, on-disk, single node, gRPC client, loopback-bound. |
| HTTP framework | `axum` + `tower` + `axum-prometheus`. |
| Viewport cross-build | `cargo-xwin` targeting `x86_64-pc-windows-msvc`; Tauri 2 + SvelteKit static adapter. |
| Auth | Bearer token from config file; constant-time compare; no rotation in MVP. |
| Dedupe | SHA-256 of canonical JSON (facts) / normalized text (knowledge). |
| Migrations | `sqlx` migrate, auto-applied at service startup. |
| Storage root | Single configurable path (`KLAMS_DATA_ROOT`, default `/ai/klams/data`) for all stateful service volumes; runtime config generated by `/speckit.tasks`. |
| Docker network | Single user-defined bridge `klams-net` shared by all service containers; DNS aliases per service. |

## Phase 1 — Design & Contracts

Artifacts produced:

- [data-model.md](data-model.md) — entity schemas, Postgres DDL, Qdrant
  payload, `MemoryWrite` enum, search-result and health-snapshot shapes.
- [contracts/openapi.yaml](contracts/openapi.yaml) — full HTTP contract
  for every MVP endpoint (FR-001..009, 016..018).
- [contracts/viewport-commands.md](contracts/viewport-commands.md) —
  Tauri command surface exposed to the Svelte frontend.
- [quickstart.md](quickstart.md) — fresh-checkout build + run + test
  walkthrough for both service and viewport, and the documented
  pre-commit gate.
- Agent context updated: `.github/copilot-instructions.md` SPECKIT
  block points to this plan.

**Post-design constitution re-check**: PASS — no new principles
violated; contracts are minimal, no speculative endpoints, no
abstractions added beyond what the user stories demand.

## Phase 2 — Tasks

Not produced by `/speckit.plan`. Run `/speckit.tasks` next to break
this plan into ordered, dependency-aware tasks in `tasks.md`.
