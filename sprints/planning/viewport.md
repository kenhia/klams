# klams memory viewport

**Status:** Planning
**Stack:** Tauri 2 (Rust backend) + Svelte 5 (frontend)
**Target OS:** Windows (primary). Linux dev machines (`kai`, `kubs0`) are
headless, so the viewport runs on Ken's Windows workstation.
**Companion document:** [plan.md](plan.md)

## 1. Purpose

The viewport is the human-facing window into klams. It is built **from
the start** because Linux dev machines are headless, leaving no GUI to
inspect state during debugging. The viewport gives Ken a native desktop
app on Windows that, across phases, grows to cover:

1. **klams memory** (Phase 1 onward) — list and inspect facts, events,
   and knowledge items; delete or override; view provenance.
2. **klams context preview** (Phase 4 onward) — preview what an agent
   would receive from `/memory/context` for a query.
3. **Agent activity** (Phase 6 onward) — view recent agent proposals,
   accepted and rejected.

Work-item management is **out of scope** — Ken already has a separate
app for kwi.

The viewport is **a debugging tool first**. UI polish is secondary to
exposing the underlying state faithfully.

## 2. Architecture

### 2.1 Process model

```
Windows desktop
└── klams-viewport.exe  (Tauri shell)
    ├── Rust backend  (tauri::command handlers)
    │     └── klams-client     — HTTP client to memory service on kubs0
    └── Svelte frontend  (SvelteKit + Vite, static SPA)
          ├── Memory view
          ├── Context preview view
          └── Agent activity view
```

### 2.2 Repo layout

```
viewport/
  src-tauri/                 # Rust backend
    Cargo.toml
    src/
      main.rs
      commands/
        memory.rs            # tauri::command wrappers around klams-client
  src/                       # Svelte frontend
    routes/
      +layout.svelte
      +page.svelte           # dashboard
      memory/
      context/
      activity/
    lib/
      api.ts                 # invoke() wrappers
  package.json
  svelte.config.js
  tauri.conf.json
```

The `klams-client` Rust crate lives under the top-level `crates/` so it
can be shared with the memory service and any CLI tools.

### 2.3 Configuration

A single config file `%APPDATA%/klams/viewport.toml` holds:

```toml
[klams]
url = "http://kubs0:7777"
token = "..."
```

Auth tokens are stored via the OS credential manager (`keyring` crate)
when possible; the TOML holds only references.

## 3. Phase 0 — Scaffold

**Goal:** Buildable Tauri + Svelte app with a placeholder window that
launches on Windows.

Deliverables:

1. `viewport/` initialized with `npm create tauri-app@latest`
   (Svelte + TypeScript template), then upgraded to Svelte 5 / SvelteKit
   if not default.
2. Tauri config: app name `klams-viewport`, identifier
   `dev.ken.klams.viewport`, single window 1200×800.
3. Cross-compilation or remote build path documented:
   - Option A: build on Windows directly (preferred; just `pnpm tauri build`).
   - Option B: cross-build from Linux via `cargo-xwin` (documented as
     a fallback).
4. CI: `pnpm install`, `pnpm check`, `cargo fmt --check`,
   `cargo clippy -- -D warnings` for `src-tauri/`.
5. README at `viewport/README.md` covering build and run on Windows.
6. Placeholder `+page.svelte` showing app name, version, and connection
   status placeholders.

Exit criteria: a Windows MSI/EXE installs `klams-viewport`, launches,
and shows the placeholder dashboard.

## 4. Phase 1 — klams memory inspector

**Goal:** See klams memory end-to-end, so the MVP service is debuggable
without a terminal session on `kubs0`.

Deliverables:

1. `klams-client` crate (Rust): typed wrappers for the Phase 1 read
   endpoints (`GET /memory/facts`, `POST /memory/search`,
   `GET /healthz`).
2. Tauri commands for memory read operations.
3. Svelte UI:
   - Facts view: filter by `type`, `source`, time range; columns for
     payload preview, `confidence`, `decay_weight`, `last_used_at`,
     `use_count`.
   - Events view: filter by `task_id`, `category`, time range.
   - Knowledge view: search box → vector + filter results; click for
     full text and metadata.
   - Per-item detail pane with full payload and copy-id action.
   - Connection status indicator (health + last refresh).
4. `viewport/docs/memory.md` usage guide.

Exit criteria: Ken can find any item shown in the Phase 1 memory
service via the viewport and inspect its full payload.

## 5. Phase 2 — write operations and provenance

**Goal:** Mutate memory from the viewport once the service supports it.

Deliverables:

1. Extend `klams-client` with the Phase 2 admin endpoints
   (`DELETE /memory/facts/:id`, `POST /memory/facts/:id/override`).
2. Per-item actions in the Svelte UI: delete, override (with
   confirmation dialog).
3. Provenance panel: source, created/updated timestamps, version
   history (if available).
4. Optimistic UI updates with rollback on backend error.

Exit criteria: Ken can delete or override a fact and the change is
reflected on next read.

## 6. Phase 4 — context preview

**Goal:** Preview what an agent would receive for a query.

Deliverables:

1. Svelte UI: query input + token-budget slider → calls
   `POST /memory/context` and renders the returned bundle
   (facts, knowledge, events) with section headers and a token-count
   readout per section.
2. Side-by-side raw vs summarized toggle.

Exit criteria: a representative query shows a coherent, budget-respecting
context bundle.

**Sprint 005 delivery (US5):** Available at the `/preview` route,
backed by:

- `viewport/src/lib/api/context.ts` — typed `contextApi.fetch()`
  wrapper over the Tauri `memory_context` command (which proxies
  `klams_client::Client::memory_context`, reusing the existing
  bearer/base-URL plumbing).
- `viewport/src/lib/components/ContextPreview.svelte` — query
  box, 250 ms-debounced token-budget slider
  (sprints/005-advanced-retrieval/research.md D-009), per-section
  status pills (`ok` / `degraded` / `unavailable`), and a
  raw-vs-summarized toggle that re-fetches without losing
  query/budget state.
- 503 + `Retry-After` from `/memory/context` (all retrieval sources
  unavailable, FR-011) surfaces as a banner above the bundle.

## 7. Phase 6 — agent activity panel

**Goal:** Surface agent proposals and outcomes.

Deliverables:

1. Backend endpoint (added in memory service Phase 6) listing recent
   agent writes with their accept/reject status and reason.
2. Svelte UI: chronological feed with filters by agent, status, and
   memory type; clickable rows for full payload + validator output.

Exit criteria: a GHCP-driven write appears in the panel within a few
seconds, with its status visible.

## 8. Build, distribution, and updates

- **Build:** `pnpm tauri build` on Windows produces an MSI installer
  in `viewport/src-tauri/target/release/bundle/msi/`.
- **Signing:** Out of scope for personal use; revisit if SmartScreen
  warnings become annoying.
- **Distribution:** Copy the installer to Ken's Windows workstation
  manually for early phases. A Tauri updater can be added later (see
  backlog).
- **Versioning:** `viewport/tauri.conf.json` `version` follows
  `MAJOR.MINOR.PATCH`. Bumped each phase that ships viewport changes.

## 9. Open questions

| Question | Decided at |
|---|---|
| SvelteKit static adapter vs plain Vite + Svelte? | Phase 0 planning |
| Use Tauri 2.x stable (assume yes) | Phase 0 |
| Self-update mechanism | Backlog |
| Theme / branding | Deferred until after Phase 2 |
