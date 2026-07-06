# Implementation Plan: Safety, Drift Control, and the User View

**Branch**: `002-safety-and-write-ops` | **Date**: 2026-05-17 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/sprints/002-safety-and-write-ops/spec.md`

## Summary

Phase 2 of [planning/plan.md](../planning/plan.md). On top of the 001
MVP we add five concurrent slices: (1) per-`Fact.type` schema
validation with universal sanity rules, (2) source-trust enforcement
with a **dissents** store that preserves rather than discards
contradictory lower-trust writes, plus optimistic-concurrency
versioning, (3) a background decay task that recomputes
`decay_weight` from per-type λ values in service config, (4) viewport
write/delete/override actions, a provenance panel, and a dedicated
dissents-review surface, and (5) a `justfile` at the repo root that
captures the developer inner loop and is the single command CI uses
for the pre-commit gate.

The sprint adds one new Postgres table (`dissents`) plus a
`dissent_count` denormalized counter on `facts` maintained by
triggers, four new endpoints under `/memory/dissents`, an extended
`HTTP 202 dissent` response on `POST /memory/facts`, and a
`dissent_count` field on default fact reads / search hits. No new
external dependencies; no new crates. The systemd switchover from
`just run` to `systemctl` is **deferred to sprint 003** (Phase 3 in
the planning doc) — this sprint keeps the dev workflow on `just run`
and the production stack on Compose.

## Technical Context

**Language/Version**:

- Service & shared crates: Rust stable, pinned via `rust-toolchain.toml`
  (unchanged from 001).
- Viewport backend: Rust (same toolchain), Tauri 2.x (unchanged).
- Viewport frontend: TypeScript + Svelte 5 (SvelteKit static adapter)
  built with Vite (unchanged).

**Primary Dependencies**:

- Service (already in workspace): `tokio`, `axum`, `tower`/`tower-http`,
  `serde`, `sqlx` (Postgres, compile-time-checked), `qdrant-client`,
  `reqwest`, `tracing` + `tracing-subscriber`, `axum-prometheus`,
  `uuid`, `time`, `thiserror`, `subtle`.
- New (this sprint): `jsonschema` is **not** added — per-type
  validators are hand-written Rust functions per [research.md §1](research.md#1-per-type-validator-strategy);
  no new crates land in `Cargo.toml`.
- New developer tooling: [`just`](https://github.com/casey/just) (recipe
  runner). Install instructions go in `docs/setup.md`; not a Rust
  dependency.

**Storage**:

- PostgreSQL 16 (unchanged stack from 001): one new table `dissents`,
  one new column `dissent_count INT NOT NULL DEFAULT 0` on `facts`
  maintained by triggers, plus indexes on `dissents (status, fact_id)`
  and `dissents (fact_id, payload_hash) WHERE status = 'pending'`.
  Migration ships as `migrations/0002_dissents.sql`.
- Qdrant: no schema change. Decay updates land in Postgres only; the
  Qdrant `decay_weight` payload field stays at MVP defaults and is
  out of scope for this sprint (knowledge-item decay tracked in the
  backlog).
- Service config: `deploy/config/klams.example.toml` gains a
  `[decay]` section with per-`type` λ values (`UserFact`, `TaskFact`,
  `EnvFact`) and a `task_interval_seconds` knob. Defaults are baked
  into the loader for any type missing from config.

**Testing**:

- `cargo test` per crate (unchanged).
- Integration tests under `crates/klams-service/tests/` run against the
  same ephemeral Compose stack at `tests/docker-compose.test.yml`. New
  files this sprint: `us1_validation.rs`, `us2_dissents.rs`,
  `us3_decay.rs`. The Phase 1 integration tests
  (`us1_facts.rs` … `us5_health.rs`, `perf_smoke.rs`) MUST continue to
  pass without modification beyond the `dissent_count` field appearing
  on `Fact` responses.
- New contract tests under `crates/klams-api/tests/`:
  `contract_dissents.rs` and an extension of `contract_facts.rs` for
  the HTTP 202 dissent response shape and HTTP 409 version-conflict
  body.
- Viewport: extend the existing Vitest `lib/api.ts` suite with the
  dissent endpoints; add a Tauri-command unit test for the dissent
  promote/discard wrappers using the existing trait-mocked
  `klams-client`.
- The constitution pre-commit gate (`cargo fmt --check`, `cargo clippy
  --workspace -- -D warnings`, `cargo test --workspace`) runs via
  `just gate` and is what CI invokes — see FR-026 and SC-006.

**Target Platform**:

- Service: Linux x86_64 on `kubs0`, **run via `just run` for this
  sprint** (dev) and `cargo run -p klams-service --release` in
  Compose-adjacent foreground for manual smoke. The systemd
  installation step from the unit file at `deploy/systemd/klams.service`
  is explicitly **deferred to sprint 003** (planning/plan.md §8 Phase 3).
- Viewport: Windows 10/11 x86_64, single `klams-viewport.exe`,
  cross-built on Linux via `cargo-xwin` targeting
  `x86_64-pc-windows-msvc` (unchanged from 001).

**Project Type**: Multi-component workspace — backend service + desktop
GUI client. No structural change from 001; the existing crate layout
(`klams-types`, `klams-core`, `klams-store`, `klams-api`,
`klams-service`, `klams-client`) absorbs every new module this sprint
introduces.

**Performance Goals**:

- Inherits SC-003 from 001: unified search p95 < 500 ms at the MVP
  corpus size (10k facts / 50k events / 10k knowledge items).
- New constraint: the decay background task MUST NOT block reads. It
  runs as a separate `tokio::spawn` task, uses small `LIMIT` batches
  (default 500 facts/iteration), commits each batch in its own
  transaction, and yields between batches. See
  [research.md §4](research.md#4-decay-task-design).
- New constraint: dissent persistence on a contradictory write MUST
  add no more than ~20 ms to the existing canonical-write path on the
  test stack — same Postgres transaction, no extra round trips.

**Constraints** (delta from 001):

- LAN-only deployment, bearer token, single host on `kubs0` — unchanged.
- Promote and discard endpoints MUST reject any source whose trust is
  below `Controller` (i.e. only `User` and `Controller` may promote or
  discard). Enforced at the API layer by inspecting the request's
  declared source against an allowlist.
- Dissent dedupe (FR-013) uses SHA-256 of canonical JSON over the
  proposed payload, the proposing source, and the canonical fact id —
  identical to the existing dedupe convention for facts.
- Decay λ defaults if a type is unconfigured: `UserFact` = 1e-9
  (effectively no decay), `TaskFact` = 1e-6, `EnvFact` = 1e-9, with
  `task_interval_seconds = 3600`. Service start MUST log resolved
  values per type at INFO.

**Scale/Scope** (unchanged from 001 MVP target):

- ~10k facts, ~50k events, ~10k knowledge items.
- One concurrent writer (the controller) plus the viewport as a
  reader; ≤ 5 concurrent in-flight requests typical.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Verdict | Notes |
|---|---|---|
| I. Spec-Driven Development | PASS | Every section of this plan derives from a numbered FR/SC in [spec.md](spec.md). The dissent design and decay task were called out in 001's deferred items and parent [planning/plan.md](../planning/plan.md) §8 Phase 2. |
| II. Test-Driven Development | PASS | New integration tests (`us1_validation.rs`, `us2_dissents.rs`, `us3_decay.rs`) and contract tests (`contract_dissents.rs`) land before or alongside implementation. The dissent dedupe and HTTP 409 version-conflict body shapes are specified in the OpenAPI delta before the handler is written. |
| III. Code Standards Gate | PASS | `just gate` runs `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` — this is FR-026 and SC-006. CI invokes the same command (no separate CI script). |
| IV. Documentation | PASS | `docs/setup.md` gains a `just` install snippet and the `[decay]` config block. `docs/usage.md` documents the dissent lifecycle, the promote/discard flows, and the viewport's provenance panel + dissents view. `docs/architecture.md` adds the dissent path to the pipeline diagram and a short "Phase 2 deltas" section. README gets a `just --list` blurb. |
| V. Quality & Observability | PASS | New metrics: `klams_validation_rejections_total{rule}`, `klams_version_conflicts_total`, `klams_dissents_total{outcome}` where outcome ∈ `accepted,duplicate,promoted,discarded,orphaned`, `klams_decay_runs_total`, `klams_decay_facts_updated_total`. Structured log fields per FR-014. Errors stay actionable (extended envelope adds `details: [{field, rule, value?}]`). |
| VI. Simplicity & Intentional Design | PASS | No new crates; no JSON-schema library; no per-source rate limiting (deferred to Phase 6); no audit log table (the dissent record is the audit trail for promote/discard, see [research.md §3](research.md#3-dissent-storage-shape)); no `override_user_fact` flag (the dissent pathway supersedes the early planning-doc sketch — see [research.md §2](research.md#2-conflict-resolution-via-dissents-rather-than-an-override-flag)). |

No violations to track in the Complexity Tracking section.

**Re-check after Phase 1 design**: PASS — the data-model, contracts,
and quickstart introduce no new abstractions beyond what the
acceptance scenarios require. No new principles violated.

## Project Structure

### Documentation (this feature)

```text
sprints/002-safety-and-write-ops/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── openapi.yaml     # Phase 2 delta over 001's contract (full file)
├── checklists/
│   └── requirements.md  # (existing) spec quality checklist
└── tasks.md             # Phase 2 output (NOT created by /speckit.plan)
```

### Source Code (repository root)

```text
Cargo.toml                  # workspace root (unchanged; no new crates)
justfile                    # NEW — FR-024..030
rust-toolchain.toml         # unchanged
crates/
  klams-types/
    src/
      lib.rs                # +Dissent, +DissentStatus, +DissentSubmittedResponse,
                            # +VersionConflict (wire), +ValidationError details,
                            # +UpsertFactResponse variants
  klams-core/
    src/
      validate/             # NEW module
        mod.rs              # registry: type -> Validator fn
        sanity.rs           # universal sanity rules (timestamp range, hostname, ranges)
        facts.rs            # per-FactType validators (UserFact, TaskFact, EnvFact)
        events.rs           # per-category validators where meaningful
        knowledge.rs        # text-length, tag rules (already partly in store/embeddings)
      decay.rs              # NEW — DecayConfig, decay loop, scoring helper
      worker.rs             # +dissent path, +version check, +last_used_at bump
      queue.rs              # (oneshot return type extended to carry dissent result)
  klams-store/
    src/
      postgres.rs           # +dissents CRUD, +fact version check, +dissent_count
                            # +decay batch update
  klams-api/
    src/
      handlers/
        facts.rs            # +HTTP 202 dissent response, +HTTP 409 version body
        dissents.rs         # NEW — list, get, promote, discard
        search.rs           # +dissent_count on hits
      error.rs              # +Conflict {current_version}, +Validation details list
      router.rs             # +/memory/dissents routes
    tests/
      contract_facts.rs     # extend with 202+409 cases
      contract_dissents.rs  # NEW
  klams-service/
    src/
      config.rs             # +[decay] section, +decay defaults
      main.rs               # spawn decay task; wire validator registry
    tests/
      us1_validation.rs     # NEW
      us2_dissents.rs       # NEW
      us3_decay.rs          # NEW
      # existing us{1..5}_*.rs and perf_smoke.rs MUST still pass
  klams-client/
    src/
      lib.rs                # +list_dissents, +get_dissent, +promote_dissent,
                            # +discard_dissent, +upsert_fact returns enum
                            # (Persisted{Fact} | Dissented{DissentId})
migrations/
  0001_init.sql             # unchanged
  0002_dissents.sql         # NEW — dissents table, dissent_count column,
                            # triggers, indexes
deploy/
  config/
    klams.example.toml      # +[decay] section
viewport/
  src/
    routes/
      dissents/+page.svelte # NEW — global dissents review surface
      facts/+page.svelte    # provenance panel, edit/delete actions
      events/+page.svelte   # provenance panel
      knowledge/+page.svelte # provenance panel
    lib/
      api.ts                # +dissent endpoints, +promote/discard
      types.ts              # +Dissent, +UpsertResult enum, +ProvenanceBundle
  src-tauri/
    src/
      commands/
        memory.rs           # +list_dissents, +promote_dissent, +discard_dissent,
                            # +delete_fact, +edit_fact passthroughs
docs/
  architecture.md           # +dissent path in pipeline, +decay task box, +Phase 2 deltas
  setup.md                  # +`just` install, +[decay] config snippet
  usage.md                  # +dissents lifecycle, +viewport curation flow,
                            # +`just` recipe quick reference
```

**Structure Decision**:
Unchanged from 001. No new crates, no relocated modules. The dissent
machinery is a normal feature inside the existing crate boundaries
(`klams-types` for DTOs, `klams-core` for the validate+worker
pipeline, `klams-store` for persistence, `klams-api` for routes,
`klams-service` for wiring, `klams-client` for the typed wrapper).

The viewport gains exactly one new top-level route (`/dissents`) and
extends the existing facts/events/knowledge inspector pages with a
shared `ProvenancePanel` component under `viewport/src/lib/`.

## Complexity Tracking

No constitution violations to justify.

## Phase 0 — Research

See [research.md](research.md). Items resolved there:

| Decision | Outcome (summary) |
|---|---|
| Per-type validator strategy | Hand-written Rust functions in `klams-core::validate`, registered into a `HashMap<FactType, Vec<Validator>>`. No JSON-schema crate. Universal sanity rules are a separate layer that runs for every write. |
| Conflict resolution shape | Dissent store replaces the early `override_user_fact` flag sketch from the planning doc; lower-trust contradictions are persisted, not rejected; promote is the explicit override path. |
| `dissents` storage shape | One Postgres table; `dissent_count` denormalized on `facts` via INSERT/UPDATE triggers; `payload_hash` for dedupe; `status` enum {pending, promoted, discarded, orphaned}. |
| Decay task design | Single `tokio::spawn` task, batched UPDATE with bind-array of ids, per-type λ from config, `task_interval_seconds` default 3600, never blocks reads. |
| `last_used_at` write coalescing | Best-effort fire-and-forget bump in a small `tokio::mpsc` channel drained by the decay task's idle slots; lost updates are acceptable (use_count is a coarse signal). |
| Viewport optimistic update + rollback | Svelte stores hold a pending-mutation queue; on backend error, the store rewinds and surfaces the API error envelope. |
| `justfile` recipe set | The eleven recipes listed in FR-025; `gate` is the constitution command from `.specify/memory/constitution.md`. |
| systemd deferral | Defer install to sprint 003 per parent planning doc; `just run` is the supported dev path; the unit file at `deploy/systemd/klams.service` is documented as "present but uninstalled". |

## Phase 1 — Design & Contracts

Artifacts produced in this Phase:

- [data-model.md](data-model.md) — `dissents` table DDL, `facts`
  delta (`dissent_count` column + triggers), validator and sanity-rule
  data shapes, decay-config shape, and the extended `MemoryWrite`
  result type.
- [contracts/openapi.yaml](contracts/openapi.yaml) — full HTTP
  contract for Phase 2, including:
  - `POST /memory/facts`: adds `200 Persisted` + `202 Dissented` +
    `409 VersionConflict` response variants.
  - `GET /memory/facts` / `GET /memory/facts/{id}`: `dissent_count` on
    every `Fact`.
  - `POST /memory/search`: `dissent_count` on every `SearchHit` whose
    `type = fact`.
  - `GET /memory/dissents`, `GET /memory/dissents/{id}`,
    `POST /memory/dissents/{id}/promote`,
    `POST /memory/dissents/{id}/discard`.
  - `ApiError` envelope extension: optional `details` array of
    `{field, rule, value?}` entries; new error code
    `version_conflict` with `current_version` field.
- [quickstart.md](quickstart.md) — `just compose-up && just run` happy
  path, sending a validation-rejected write, sending a dissent,
  listing it via `GET /memory/dissents`, promoting it via curl (and
  the viewport equivalent), observing the canonical change and the
  `dissent_count` returning to zero.
- Agent context updated: `.github/copilot-instructions.md` SPECKIT
  block now points at this plan.

**Post-design constitution re-check**: PASS — no new principles
violated; the contracts add only what the user stories demand; the
data model adds one table, one column, one migration, no shadow
schemas. No abstractions added beyond the validator registry, which
is the smallest shape that satisfies FR-002.

## Phase 2 — Tasks

Not produced by `/speckit.plan`. Run `/speckit.tasks` next to break
this plan into ordered, dependency-aware tasks in `tasks.md`.
