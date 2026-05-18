# Research: Safety, Drift Control, and the User View

This document resolves the design choices the spec deferred to the
plan phase. Each section follows the Decision / Rationale /
Alternatives format.

## 1. Per-type validator strategy

**Decision**: Implement validators as hand-written Rust functions
registered in a `HashMap<FactType, Vec<ValidatorFn>>` inside
`klams-core::validate`. A separate `sanity` module holds universal
rules (timestamp range, hostname shape, numeric range) applied to
every write regardless of source. No external schema library.

**Rationale**:

- The validator population for the MVP type set (`UserFact`,
  `TaskFact`, `EnvFact`, plus a couple of event categories) is small
  enough that hand-rolled Rust functions are clearer and easier to
  test than a JSON-schema indirection.
- Hand-rolled validators can produce the exact field-level error shape
  FR-003 requires (path, rule, offending value) without translating
  out of a third-party error type.
- Sanity rules are inherently Rust (timestamp clock comparison,
  regex-style hostname check, numeric range against config-driven
  bounds); duplicating them in a schema language would be a step
  backwards.
- Constitution VI (YAGNI): zero new dependencies.

**Alternatives considered**:

- `jsonschema` crate (RFC 8259 / Draft 2020-12). Rejected: every
  validator would also need a Rust-side sanity wrapper for hostname /
  timestamp / range rules, so the schema layer adds indirection
  without removing code.
- `garde` / `validator` derive macros. Rejected: the payload type is
  `serde_json::Value`, not a strongly-typed struct, so derive-style
  validation does not apply without first deserializing into per-type
  structs — which we may do later, but not in this sprint.
- Run validators inside the worker only. Rejected: the API must
  return HTTP 422 with field detail synchronously per FR-003 (it
  cannot be a `202`-then-discover-later result). Validators therefore
  run in the API handler before enqueue; the worker re-runs them as
  a defense-in-depth check.

## 2. Conflict resolution via dissents rather than an override flag

**Decision**: Replace the early planning sketch of an
`override_user_fact` boolean (planning/plan.md §6) with a dedicated
**dissents** path. Lower-trust contradictory writes are persisted as
proposals, surfaced via dedicated endpoints, and resolved by promote
(canonical write) or discard. The override pathway is "promote the
dissent" — no flag on the canonical-write call.

**Rationale**:

- Throwing away every contradiction assumes the higher-trust source
  is permanently correct, which the spec explicitly rejects in Story
  2's framing.
- Keeping dissents as first-class records preserves provenance: who
  proposed what, when, how many times. An `override_user_fact` flag
  would have produced silent overwrites with no audit trail.
- The promote/discard pair is small and explicit; the alternative
  ("reject with `409 OverrideRequired` then expect the caller to
  re-send with a flag") makes correctness depend on caller behavior.
- It composes with optimistic concurrency cleanly: promote walks the
  same `version` check path as a canonical write.

**Alternatives considered**:

- Keep `override_user_fact` from the planning sketch. Rejected per
  above; the spec's Story 2 acceptance scenarios explicitly require
  proposals to be retrievable later.
- Persist contradictions as `facts` rows with a separate `status`
  column and filter them out of default reads. Rejected: it makes the
  primary table polymorphic (canonical vs proposal vs orphaned), and
  every existing read query would need a `WHERE status = 'canonical'`
  clause. A separate `dissents` table is simpler and keeps the hot
  read path untouched.
- A generic "approvals" framework over every write. Rejected as YAGNI
  for this sprint: there is one promotion target (lower-trust
  contradicts higher-trust) and two terminal states (promoted /
  discarded). Generalize later if a second use case appears.

## 3. Dissent storage shape

**Decision**: One Postgres table `dissents` (full DDL in
[data-model.md](data-model.md)). Columns: `id UUID PK`, `fact_id UUID
NOT NULL REFERENCES facts(id) ON DELETE CASCADE`, `proposed_payload
JSONB NOT NULL`, `payload_hash BYTEA NOT NULL`, `source TEXT NOT NULL`
(the proposing source), `submitted_at TIMESTAMPTZ NOT NULL DEFAULT
now()`, `last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()`,
`submission_count INT NOT NULL DEFAULT 1`, `status TEXT NOT NULL
DEFAULT 'pending'` (CHECK in `{pending, promoted, discarded,
orphaned}`), `resolved_at TIMESTAMPTZ NULL`, `resolved_by_source TEXT
NULL`. A partial UNIQUE index on `(fact_id, payload_hash) WHERE status
= 'pending'` implements FR-013's dedupe.

`facts` gains `dissent_count INT NOT NULL DEFAULT 0` maintained by a
pair of `AFTER INSERT` / `AFTER UPDATE OF status` triggers on
`dissents` that recompute the count from
`COUNT(*) FILTER (WHERE status='pending')`. The trigger uses a single
UPDATE; the volume is low (typically zero pending per fact) so the
cost is negligible.

**Rationale**:

- A separate table keeps the canonical-read hot path untouched and
  lets us add dissent-only indexes (status, age) without polluting
  `facts`.
- `ON DELETE CASCADE` plus a trigger that flips remaining pending
  rows to `orphaned` BEFORE the cascade fires (a `BEFORE DELETE`
  trigger on `facts`) implements the edge-case requirement that a
  deleted canonical fact must produce deterministic dissent state.
- `dissent_count` as a column (rather than a view or a per-request
  aggregate) keeps `GET /memory/facts` fast and indexable without
  joins.
- `payload_hash` is the same SHA-256-of-canonical-JSON convention the
  facts table already uses (see [001-initial-mvp/data-model.md](../001-initial-mvp/data-model.md#fact-postgres-facts)),
  so dedupe is consistent across the codebase.

**Alternatives considered**:

- Compute `dissent_count` as a generated column with a subquery.
  Postgres does not allow subqueries in generated columns, so this is
  not possible.
- Compute it as a materialized view refreshed periodically. Rejected:
  adds operational burden (refresh scheduling), adds staleness, and
  trigger maintenance is trivially cheaper at this volume.
- Compute it on read with `LEFT JOIN LATERAL`. Rejected: every
  fact-list endpoint pays the join cost; the spec explicitly wants
  `dissent_count` on default reads (FR-012).
- Store dissents in JSONB inside `facts.payload` under a reserved key.
  Rejected: violates payload-as-data principle; makes querying for
  "all pending dissents older than X" effectively impossible without
  a full table scan.

## 4. Decay task design

**Decision**: A single `tokio::spawn`ed task started by `klams-service`
at startup. Loop: sleep for `task_interval_seconds`, then walk facts
in batches of 500 ordered by `id`, computing
`decay_weight = base × (1 / (1 + λ_type × age_seconds))` where
`age_seconds = now() − last_used_at` (falling back to `created_at`).
Each batch is one UPDATE … FROM (VALUES …). Between batches, `yield_now().await`.
Per-type λ pulled from the loaded `DecayConfig`; missing types use the
default-table fallback documented in plan §"Constraints".

**Rationale**:

- One task with explicit batching keeps the read path untouched: the
  background work holds no row locks beyond the batch UPDATE
  duration.
- `id`-ordered iteration is deterministic and resumable (the task
  records the last processed id between batches, so a restart picks
  up roughly where it left off without state in the table).
- Computing in Rust and bulk-UPDATEing avoids a complex SQL CASE
  expression per type while still letting Postgres apply the writes
  efficiently.
- A single task (not one per type) avoids contention between
  concurrent UPDATEs on the same `facts` row.

**Alternatives considered**:

- Compute `decay_weight` inline at read time. Rejected: makes search
  ranking depend on a per-row computation in the query, which
  complicates the existing hybrid scoring and costs more on every
  read.
- Compute as a generated column. Rejected: `now()` is not allowed in
  generated columns and λ is per-type config, not a constant.
- One task per type. Rejected as YAGNI; the per-type loop inside one
  task is clearer and avoids cross-task locking concerns.

## 5. `last_used_at` write coalescing

**Decision**: Reads that successfully return a fact send an `id` over
a small bounded `tokio::mpsc::Sender<Uuid>` (capacity 1024). The decay
task (or a sibling lightweight task spawned next to it) drains the
channel between batches and issues a single
`UPDATE facts SET last_used_at = now(), use_count = use_count + 1 WHERE id = ANY($1)`.
If the channel is full, the bump is dropped (fire-and-forget); a
metric `klams_last_used_bumps_dropped_total` tracks loss.

**Rationale**:

- Bumping `last_used_at` synchronously on every read costs a Postgres
  round trip per read and would regress SC-003.
- A bounded channel with drop-on-full bounds the worst case (no
  unbounded growth, no read-path blocking).
- `use_count` is documented in the spec as a coarse usefulness signal;
  losing some increments under load is acceptable.

**Alternatives considered**:

- Trigger-based update inside the SELECT. Postgres triggers do not
  fire on SELECT; only on row mutations.
- Defer to the decay task's own pass (skip per-read bumping). Rejected:
  loses the "frequently-accessed memory resists decay" behavior
  required by FR-018 and Story 3 Acceptance Scenario 4.
- Use Redis or a sidecar buffer. Rejected: adds a dependency for a
  best-effort counter.

## 6. Viewport optimistic update with rollback

**Decision**: Each mutating viewport action (delete, edit, promote,
discard) follows the pattern: (a) snapshot the current Svelte store
slice for the affected entity, (b) apply the predicted post-mutation
state to the store and re-render, (c) issue the backend call, (d) on
success, refresh the list/detail to reconcile with authoritative
state; on failure, restore the snapshot and surface the API error's
`message` and (where present) `details` array in a toast.

**Rationale**:

- The viewport's existing facts/events/knowledge pages already use
  per-page Svelte stores; piggybacking on them is the smallest change.
- Snapshots in JS memory are cheap; the affected slices are small.
- Reconciliation after success catches server-side mutations (e.g. a
  trigger-driven `dissent_count` change) without a second predictive
  step.

**Alternatives considered**:

- No optimism (wait for backend round-trip). Rejected: degrades
  perceived latency on a LAN where backend round-trip is dominated by
  network jitter, not server work.
- Generic command-queue/undo-stack architecture. Rejected as YAGNI;
  per-page snapshot-and-restore is sufficient for the actions in scope.

## 7. `justfile` recipe set

**Decision**: A `justfile` at the repo root with exactly the recipes
named in FR-025. `gate` runs the three constitution pre-commit
commands in order, failing fast. `compose-up`/`compose-down`/
`compose-rebuild` shell out to `docker compose -f deploy/docker-compose.yml`.
`run` is `cargo run -p klams-service` with logs to stderr. `health`
curls `/healthz` and invokes `scripts/verify-mvp.sh --light` (the
script gains a `--light` flag if not already present). `verify` is
the full `scripts/verify-mvp.sh`. `viewport-build` is
`cd viewport && pnpm tauri build --target x86_64-pc-windows-msvc --bundles none`
behind the existing cargo-xwin environment.

**Rationale**:

- Constitution III demands one canonical pre-commit gate; `just gate`
  is that command and CI calls exactly the same thing.
- A `justfile` is dependency-light (`just` is a single static
  binary), well-known, and works identically on dev and CI.
- Idempotency for `compose-up` is handled by Compose itself
  (`up -d`); the recipe trusts Compose's behavior rather than
  reimplementing it.

**Alternatives considered**:

- `make`. Rejected: tab whitespace pitfalls, weaker variable handling,
  no built-in recipe listing equivalent to `just --list`.
- A shell script per task in `scripts/`. Rejected: duplicates what
  `just` does and lacks a `--list` UX.
- Adding the recipes to a `task` runner inside `Cargo.toml`
  (`cargo xtask`). Rejected: a Rust binary for `docker compose up` is
  more code than a recipe line.

## 8. systemd deferral

**Decision**: The systemd unit at [deploy/systemd/klams.service](../../deploy/systemd/klams.service)
remains present-but-uninstalled in this sprint. The dev and CI flow
runs the service via `just run`. The install step (and the
documentation shift in `docs/setup.md` from "run via just" to "manage
via systemctl") lands in sprint 003 alongside the rest of Phase 3 in
[planning/plan.md](../planning/plan.md) §8.

**Rationale**:

- The parent planning doc explicitly schedules systemd installation
  for Phase 3 ("Install `deploy/klams-service.service` as a systemd
  unit on `kubs0`; switch the day-to-day workflow from `just run` to
  `systemctl` management…").
- This sprint's deliverables (validation, dissents, decay, viewport
  curation, justfile) are independent of the runtime supervisor.
  Coupling them would expand scope without acceptance criteria
  benefit.
- `just run` is the documented dev workflow per Story 5 Acceptance
  Scenario 4.

**Alternatives considered**:

- Install systemd this sprint. Rejected: out of scope per planning
  doc; introduces install/uninstall churn on `kubs0`; would require
  systemd-aware integration tests not currently in the suite.
