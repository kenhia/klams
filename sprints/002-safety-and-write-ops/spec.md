# Feature Specification: Safety, Drift Control, and the User View

**Feature Branch**: `002-safety-and-write-ops`  
**Created**: 2026-05-17  
**Status**: Draft  
**Input**: Operationalize Phase 2 of `sprints/planning/plan.md` on top of the 001-initial-mvp service: per-`Fact.type` schema validation, conflict resolution and source-trust enforcement, a decay model, hallucination filters for agent-sourced writes, viewport write operations + provenance panel, and a `justfile` at the repo root for the developer inner loop.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Reject malformed and untrustworthy agent writes (Priority: P1)

Ken's controller and any future agent must never be able to poison klams memory with malformed payloads or hallucinated facts. When such a write arrives, the service rejects it with a specific, actionable error and the bad data never reaches Postgres or Qdrant. Operators see the rejection in logs and metrics.

**Why this priority**: Without this gate, every later capability — search ranking, provenance, viewport curation — operates on tainted state. This is the smallest slice that makes klams safe to point at untrusted writers, which is the entire premise of Phase 2.

**Independent Test**: Send a `POST /memory/facts` payload with a missing required field, an out-of-range value, or a malformed hostname/timestamp using an `Agent` source. Confirm HTTP 422 with field-level detail in the response body, and confirm via `GET /memory/facts` and worker logs that nothing was written.

**Acceptance Scenarios**:

1. **Given** a `UserFactUpsert` payload missing a required field, **When** the client POSTs it with source `Agent`, **Then** the API returns HTTP 422 with a body identifying the missing field and the write never reaches the worker's store layer.
2. **Given** a `TaskFactUpsert` whose payload claims a hostname like `not a hostname!`, **When** the client POSTs it with source `Agent`, **Then** the API returns HTTP 422 with a sanity-rule error and the worker rejection is observable in metrics.
3. **Given** an `EventAppend` with a `created_at` more than 10 years in the future, **When** the client POSTs it with source `Agent`, **Then** the API returns HTTP 422 with a range error.
4. **Given** the same payload submitted with source `User` or `Controller`, **When** validation passes schema but trips a sanity rule, **Then** the rule still fires (sanity rules are universal); the trust difference only affects the override path in Story 2.

---

### User Story 2 - Preserve dissenting writes for human review (Priority: P1)

Facts Ken sets himself (source `User`) take precedence on normal reads, but Ken is not infallible and a fact correct today may be wrong tomorrow. When a less-trusted source (typically `Agent`) writes a contradiction to an existing higher-trust fact, the contradiction is **not silently rejected** — it is recorded in a separate dissents store that does not appear in default reads or search results. Ken (or a future curator agent session) can review dissents and either promote one to replace the canonical fact or discard it. Concurrent writers at the same trust level still get optimistic-concurrency protection so they cannot silently clobber each other.

**Why this priority**: Source-trust must be enforced for normal reads to be trustworthy, but throwing away every contradiction assumes the trusted source is permanently correct. Dissents close that loop. This pairs with Story 1 to make Phase 2 meaningful.

**Independent Test**: Write a fact with source `User`, then submit a contradictory write to the same `(type, key)` with source `Agent`. Confirm the canonical fact is unchanged and that `GET /memory/dissents?fact_id=…` returns the agent's proposal with its original payload, source, and timestamp. From the viewport's dissents view, promote the dissent and confirm it becomes the new canonical fact (version incremented, source recorded). Separately, submit two concurrent writes from the same trust level against the same `version` and confirm the second returns HTTP 409 with the current version in the body.

**Acceptance Scenarios**:

1. **Given** a fact stored with source `User` and `version=3`, **When** an `Agent`-sourced upsert targets the same `(type, key)` with a contradictory payload, **Then** the API returns HTTP 202 (accepted-as-dissent), the canonical fact is unchanged, and the proposal is queryable via the dissents endpoint with its original payload, source, and timestamp.
2. **Given** one or more dissents exist for a fact, **When** the client calls `GET /memory/facts/{id}` or `POST /memory/search`, **Then** the response reflects only the canonical fact but includes a non-zero dissent count so curators know review is pending.
3. **Given** a pending dissent, **When** Ken (or a Controller-trusted client) calls the promote endpoint with that dissent's id, **Then** the dissent's payload becomes the canonical fact, `version` increments, `source` records the promoting actor, and the dissent record is marked resolved.
4. **Given** a pending dissent, **When** Ken (or a Controller) calls the discard endpoint, **Then** the dissent is marked resolved-discarded and the canonical fact is unchanged.
5. **Given** a fact at `version=5`, **When** two writers at the same trust level each submit an upsert claiming `version=5`, **Then** the first succeeds and the second receives HTTP 409 whose body reports `current_version=6`. (Optimistic concurrency is independent of the dissent path.)
6. **Given** a Task-memory fact, **When** a newer Task write arrives, **Then** newest wins per the conflict rules (no dissent — same-trust writes use newest-wins, not dissent storage).
7. **Given** a Knowledge item being re-indexed by the same or higher trust source, **When** the new version arrives, **Then** metadata merges while the embedding and text are replaced (knowledge re-indexing is not treated as dissent).

---

### User Story 3 - Decay-aware ranking that fades stale memory (Priority: P2)

Ranking in `/memory/search` must reflect that not all memory ages the same way. A background task periodically recomputes `decay_weight` and `last_used_at` using a per-type λ loaded from the service config. Stable Machine facts barely decay; Working/ephemeral facts decay fast; Task and Knowledge memory sit in between. Search results re-order accordingly without callers changing their queries.

**Why this priority**: Without decay, search relevance degrades as the store grows and the user view fills with stale junk. This is critical for Phase 2 to feel "self-correcting", but it is independent of the safety gates in Stories 1–2 and can ship after them.

**Independent Test**: Seed two facts of equivalent text relevance — one Machine-typed, one Working-typed — and run the decay task with simulated elapsed time. Confirm the Working-memory fact's `decay_weight` drops measurably more than the Machine fact's, and that `/memory/search` returns the Machine fact higher on a query that matches both.

**Acceptance Scenarios**:

1. **Given** two facts of different types but matching a shared search query, **When** the decay task runs after simulated elapsed time, **Then** `decay_weight` is recomputed per the §7 formula using the type's configured λ.
2. **Given** the recomputed weights, **When** the client calls `POST /memory/search`, **Then** the result ordering reflects the new weights.
3. **Given** the service config sets λ for a type, **When** the service starts, **Then** the decay task picks up the configured value without code changes.
4. **Given** the decay task is running, **When** a read touches a fact, **Then** `last_used_at` updates so frequently-accessed memory resists decay.

---

### User Story 4 - Inspect and curate memory from the viewport (Priority: P2)

From the viewport, Ken can open any fact, event, or knowledge item, see its full provenance (source, version history, `created_at` / `updated_at` / `last_used_at`, `decay_weight`, `confidence`), and act on it: delete with confirmation, or override with a new payload. Changes are visible on the next read.

**Why this priority**: This is the "human curator" half of Phase 2 — the safety nets in Stories 1–3 are only useful if Ken can actually inspect and fix what slips through. Builds directly on the Phase 1 inspector.

**Independent Test**: Use the viewport to open an existing fact, confirm the provenance panel shows all listed fields, delete the fact via the UI confirmation flow, and reload the list to confirm it is gone. Repeat for an override.

**Acceptance Scenarios**:

1. **Given** a fact visible in the viewport's fact list, **When** Ken clicks it, **Then** a provenance panel displays `source`, `version`, `created_at`, `updated_at`, `last_used_at`, `decay_weight`, and `confidence`.
2. **Given** the provenance panel open, **When** Ken triggers Delete and confirms in the dialog, **Then** the viewport calls the admin delete endpoint, refresh shows the fact gone, and a failed backend call rolls the optimistic update back.
3. **Given** the provenance panel open, **When** Ken triggers Edit and submits a new payload, **Then** the viewport calls the canonical write endpoint as `User` source, the new version is visible on refresh, and any prior dissents for that fact remain in the dissents queue for separate review.
4. **Given** the global Dissents view is open, **When** pending dissents exist, **Then** each row shows the canonical fact, the proposed payload diffed against canonical, the dissent's source and age, and Promote / Discard actions.
5. **Given** an event or knowledge item, **When** Ken opens it, **Then** the same provenance panel is wired and the same actions (where backend-supported) are available.

---

### User Story 5 - One-command developer inner loop via justfile (Priority: P3)

A `justfile` at the repo root captures every routine developer action so Ken (and CI) run the same commands. `just --list` shows the menu; `just gate` is the constitution pre-commit gate (fmt + clippy + tests); `just compose-up/down/rebuild` manage the deploy stack; `just run` runs the service in the foreground for debugging; `just verify` runs `scripts/verify-mvp.sh`; `just viewport-build` cross-builds the Windows viewport.

**Why this priority**: Quality-of-life and CI consistency. It depends on nothing in Stories 1–4 and they depend on nothing in it, so it ships when convenient inside the sprint.

**Independent Test**: On a clean checkout, run `just --list` and confirm every recipe in §Functional Requirements appears. Run `just gate` and confirm it executes fmt-check, clippy-with-warnings-as-errors, and the workspace test suite, exiting non-zero on any failure. Run `just compose-up` followed by `just health` and confirm the latter reports green within 30 seconds.

**Acceptance Scenarios**:

1. **Given** a clean checkout, **When** Ken runs `just --list`, **Then** the recipes `default`, `health`, `compose-up`, `compose-down`, `compose-rebuild`, `build`, `run`, `test`, `gate`, `viewport-build`, and `verify` are all present.
2. **Given** a clean working tree, **When** CI runs `just gate`, **Then** it runs `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`, and a failure in any step fails the command.
3. **Given** Docker is available, **When** Ken runs `just compose-up`, **Then** `deploy/docker-compose.yml` starts; **When** he then runs `just health`, **Then** `/healthz` is curled and a light `scripts/verify-mvp.sh` pass is invoked, returning green within 30 seconds on a previously-warm host.
4. **Given** the dev workflow described in plan.md §8 Phase 2, **When** Ken needs the service running for manual testing, **Then** `just run` launches `klams-service` in the foreground with logs to stderr.

---

### Edge Cases

- An agent submits a write that passes schema validation but trips a sanity rule (e.g. valid JSON, but a hostname field with embedded spaces). Sanity rule must fire and produce a 422.
- Two concurrent writes from the same trusted source race; both pass validation; optimistic concurrency on `version` must reject the loser with 409 and the loser must be able to retry against the new `version` cleanly.
- A `User`-sourced fact has a pending dissent; another `Agent` submits a second contradictory write to the same canonical fact. Both dissents must be retained (deduplicated by canonical hash where identical) without partial-state corruption.
- A dissent is promoted at the same instant a concurrent canonical write lands at the same `version`. Promotion must use the same optimistic-concurrency path and the loser must observe HTTP 409 cleanly.
- A canonical fact is deleted while pending dissents reference it. Dissents must either be cascaded to a resolved-orphaned state or carried forward; behavior must be deterministic and observable.
- The decay task runs while a write is in flight to the same fact; recomputed `decay_weight` must not clobber an unrelated payload update, and vice versa.
- A fact's `last_used_at` is updated by a read while the decay task computes a new `decay_weight` from the prior value; the next decay pass must converge rather than oscillate.
- The viewport deletes a fact, then the user immediately searches; the search must reflect the deletion.
- The viewport's optimistic delete fails at the backend (e.g. 409 from a concurrent edit); the UI must roll back and display the backend error.
- An `EventAppend` arrives with a timestamp slightly in the past from an out-of-sync client; ±10y is the rejection threshold, normal clock skew must pass.
- `just compose-up` is invoked while a previous stack is still running; the recipe must be idempotent or fail loudly with a clear message.
- A `KnowledgeIndex` write replaces text and embedding but keeps non-conflicting metadata fields; merging must be deterministic.

## Requirements *(mandatory)*

### Functional Requirements

**Per-type schema validation (Story 1)**

- **FR-001**: The service MUST validate the `payload` of every `MemoryWrite` variant against a per-`Fact.type` schema before dedupe and store.
- **FR-002**: Validators MUST be pluggable functions registered per type, invoked inside the worker pipeline.
- **FR-003**: Validation failure MUST cause the API to respond with HTTP 422 and a JSON body containing field-level detail (field path, rule violated, offending value where safe to echo).
- **FR-004**: `EventAppend` and `KnowledgeIndex` payloads MUST be subject to validation where a meaningful per-type schema applies; types without a registered validator MUST accept any structurally-valid payload but MUST still pass universal checks (timestamp range, required envelope fields).
- **FR-005**: Hallucination filters for agent-sourced writes MUST include required-field checks, value-range checks, and per-type sanity rules including at least: hostnames look like hostnames, timestamps are within ±10 years of "now", numeric fields fall in declared ranges.
- **FR-006**: Sanity rules MUST apply to every source; trust level only affects the dissent path (Story 2), not validation or sanity gates.

**Conflict resolution, source trust, and dissents (Story 2)**

- **FR-007**: The service MUST implement the precedence ordering User > Controller > Task/scanner > Agent for canonical writes.
- **FR-008**: Writes MUST carry optimistic concurrency on `facts.version`; a write whose claimed `version` is not the current stored version MUST be rejected with HTTP 409 and a body containing `current_version`. Optimistic concurrency applies to canonical writes and to dissent promotion alike.
- **FR-009**: A write from a source whose trust is lower than the stored fact's `source` and whose payload contradicts the canonical fact MUST be persisted as a **dissent** rather than rejected. The canonical fact MUST remain unchanged, and the API MUST respond with HTTP 202 plus the dissent id.
- **FR-010**: Same-trust or higher-trust writes follow the canonical conflict rules: User memory "user wins", Task memory "newest wins", Knowledge memory "merge metadata, replace embedding/text". Dissents are only produced when trust is strictly lower than the stored fact's source.
- **FR-011**: The service MUST expose endpoints to (a) list pending dissents (filterable by `fact_id`, `source`, age), (b) promote a dissent (replacing the canonical fact, incrementing `version`, recording the promoting actor as `source`), and (c) discard a dissent (marking it resolved without canonical change). Promote and discard MUST be restricted to `User` or `Controller` trust.
- **FR-012**: Default reads (`GET /memory/facts`, `GET /memory/facts/{id}`, `POST /memory/search`) MUST NOT include dissents in their payloads but MUST surface a `dissent_count` field on each fact so curators know review is pending. Dissents are retrievable only via the dedicated dissents endpoints.
- **FR-013**: Dissents MUST be deduplicated against existing pending dissents for the same canonical fact when the proposed payload hashes identically; the duplicate's submission count and last-seen timestamp update instead of creating a second row.
- **FR-014**: Outcomes (validation rejection, version conflict, sanity rejection, dissent accepted, dissent promoted, dissent discarded) MUST be observable in metrics and in structured logs with the outcome reason.

**Decay model (Story 3)**

- **FR-015**: A background `tokio` task MUST periodically recompute `decay_weight` per the scoring formula in plan.md §7.
- **FR-016**: Per-type λ values MUST be loaded from the service configuration file, with reasonable defaults if a type is unconfigured.
- **FR-017**: `/memory/search` ranking MUST incorporate `decay_weight` such that recomputed weights affect result ordering on the next query.
- **FR-018**: Reads that successfully return a fact MUST best-effort update its `last_used_at` and `use_count` so that frequently-accessed memory resists decay. The implementation MAY coalesce or drop individual increments under load (e.g. when a bounded channel to the bump task is full); any dropped increments MUST be observable via a counter metric (`klams_last_used_bumps_dropped_total`). This is a coarse usefulness signal; an explicit usefulness-boost mechanism is tracked in the backlog and deferred to a later sprint.

**Viewport write operations, provenance, and dissents (Story 4)**

- **FR-019**: The viewport MUST expose Delete and Edit actions on facts, both gated by a confirmation dialog. Edits from the viewport are `User`-sourced and go through the canonical write path (not the dissent path).
- **FR-020**: The viewport MUST display a provenance panel for any selected fact, event, or knowledge item showing `source`, `version` (and history if available), `created_at`, `updated_at`, `last_used_at`, `decay_weight`, `confidence`, and `dissent_count`.
- **FR-021**: The viewport MUST provide a dissents-review surface that lists pending dissents (globally and per-fact), shows each dissent's proposed payload diffed against the canonical fact, and exposes Promote and Discard actions.
- **FR-022**: The viewport MUST apply changes optimistically with rollback on backend error and surface the backend's error detail to the user.
- **FR-023**: Provenance, write, and dissent actions MUST be wired to the existing facts, events, and knowledge inspector pages.

**Justfile (Story 5)**

- **FR-024**: A `justfile` MUST exist at the repository root.
- **FR-025**: It MUST define recipes: `default` (which calls `just --list`), `health`, `compose-up`, `compose-down`, `compose-rebuild`, `build`, `run`, `test`, `gate`, `viewport-build`, `verify`.
- **FR-026**: `gate` MUST run `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`, failing on any step's failure.
- **FR-027**: `compose-up` and `compose-down` MUST manage `deploy/docker-compose.yml`; `compose-rebuild` MUST do down → `--no-cache` build → up.
- **FR-028**: `build` MUST be `cargo build -p klams-service --release`; `run` MUST run `klams-service` in the foreground with logs going to stderr.
- **FR-029**: `health` MUST curl `/healthz` and invoke `scripts/verify-mvp.sh` in a light mode; `verify` MUST run the full `scripts/verify-mvp.sh`.
- **FR-030**: `viewport-build` MUST perform a `cargo xwin` Windows cross-build of the viewport.

**Cross-cutting**

- **FR-031**: All new error responses MUST conform to the existing API error envelope established in Phase 1 (see `crates/klams-api/src/error.rs`) extended with structured field-level detail.
- **FR-032**: The service MUST NOT regress any Phase 1 contract; all existing contract tests MUST continue to pass.

### Key Entities

- **Naming note — `Agent` vs `AgentProposal`**: Throughout this spec, **`Agent`** is shorthand for the canonical `Source` enum value **`AgentProposal`** defined in 001 (`crates/klams-types/src/entities.rs`, `sprints/001-initial-mvp/data-model.md`). The wire contract, database column, and Rust enum all use `AgentProposal`; tests, validators, and handlers MUST compare against `AgentProposal` and not against the string `"Agent"`.
- **Validator**: A function registered against a `Fact.type` (or write variant) that takes a payload and returns either acceptance or a structured list of field-level violations. The set of registered validators is the schema-validation policy.
- **Sanity rule**: A universal cross-type check (timestamp range, hostname shape, numeric range) applied alongside per-type validators.
- **Dissent**: A pending write proposal from a lower-trust source that contradicts a higher-trust canonical fact. Persisted in a separate store, excluded from default reads and search, surfaced via dedicated endpoints, and resolved by promote (becomes canonical) or discard. Carries the proposed payload, the original source, the submitting timestamp, and a duplicate-submission count.
- **Decay state**: Per-record fields `decay_weight`, `last_used_at`, `use_count` plus the per-type λ in service config; together they drive search ranking.
- **Provenance bundle**: The set of fields displayed in the viewport's provenance panel: `source`, `version` (with history if retained), `created_at`, `updated_at`, `last_used_at`, `decay_weight`, `confidence`, `dissent_count`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A malformed agent-sourced write is rejected with HTTP 422 plus field-level detail and never reaches Postgres or Qdrant, verifiable by querying both stores after the rejected request.
- **SC-002**: A `User`-sourced fact survives a contradictory `Agent`-sourced write to the same `(type, key)` — the canonical fact is unchanged and the proposal is retrievable via the dissents endpoint with original payload, source, and timestamp. Promoting that dissent (from User or Controller trust) replaces the canonical fact, increments `version`, and records the promoting actor as `source`. Default reads expose a `dissent_count` so the pending proposal is discoverable.
- **SC-003**: A write against a stale `version` returns HTTP 409 whose body reports the current `version`, and a retry against that version succeeds.
- **SC-004**: After simulated elapsed time the `decay_weight` of a Working-memory fact decreases measurably more than that of a Machine-typed fact of equivalent initial relevance, and `/memory/search` re-orders results accordingly on the next query.
- **SC-005**: From the viewport, Ken deletes a fact via the confirmation flow and the deletion is visible on the next read; the provenance panel for any selected item shows `source`, `version`, `created_at`, `updated_at`, `last_used_at`, `decay_weight`, and `confidence`.
- **SC-006**: `just gate` runs `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`, and is the single command CI invokes for the pre-commit gate.
- **SC-007**: On a previously-warm host, `just compose-up` brings the test stack up and `just health` reports green within 30 seconds. *Warm* means: container images for `postgres:16`, `qdrant/qdrant:1.18.0`, and `ghcr.io/huggingface/text-embeddings-inference:cpu-1.7` are already present in the local Docker image cache, and the named volumes for Postgres data and the TEI model cache already exist and are initialized. First-time pulls and cold model warm-up are explicitly out of scope for this 30-second budget.

## Assumptions

- Phase 1 (`sprints/001-initial-mvp/`) is shipped and provides the schema, queue, worker pipeline, and `klams-client` extension points this sprint builds on.
- The repository layout, crate boundaries (`klams-types`, `klams-core`, `klams-store`, `klams-api`, `klams-service`, `klams-client`, `viewport/`), and the pipeline diagram in `docs/architecture.md` are unchanged.
- The service config file already exists (see `deploy/config/klams.example.toml`); per-type decay λ values are added there in this sprint without altering the rest of the file's shape.
- "Now" for timestamp sanity rules is the service-process wall clock; ±10 years is interpreted against that clock.
- The viewport's existing facts / events / knowledge inspector pages are the integration points for the provenance panel and per-fact write/dissent actions; a single new top-level navigation item ("Dissents") is acceptable for the global review list.
- `just` is available on developer machines and CI runners (install instructions go in `docs/setup.md`).
- Constitution pre-commit gates (`.specify/memory/constitution.md`) define what `just gate` enforces; nothing in this sprint loosens those gates.
- systemd unit installation, Ansible callbacks, repo scanner, service monitors, hybrid retrieval, summarization, `/memory/context`, backups, dashboards, `maintenance_mode`, and the MCP server are out of scope for this sprint per the user's "Explicitly out of scope" list and move to Phases 3–6.
- `just run` remains the development workflow for this sprint; the systemd switchover happens in sprint 003.

## Phase 2 walkthrough

Per task T062, the [quickstart.md](quickstart.md) flow §4–§8 was
walked against the sprint-002 build (workspace `cargo test
--workspace --all-features` + the `#[ignore]`-gated integration
suite running against the dedicated
[`tests/docker-compose.test.yml`](../../tests/docker-compose.test.yml)
stack — same stack and same `--include-ignored` invocation that CI
exercises after `just gate`). Each row is the closest mechanical
proxy for the quickstart step that we can run from this machine
without a long-lived viewport on a Windows host.

| Step                                                    | Evidence                                                                                       | Result |
|---------------------------------------------------------|------------------------------------------------------------------------------------------------|--------|
| §1 `just compose-up` + `just health`                    | `just --list` enumerates 11 recipes; `just gate` (= `cargo fmt --check` + clippy + tests) exits 0; `/healthz` returns 200 against the local stack. | PASS   |
| §2 Phase-2 migration (`0002_dissents.sql`) applied      | `cargo test -p klams-store --tests` includes the dissents-schema sqlx tests; all pass.         | PASS   |
| §3 `just run`                                           | `cargo build -p klams-service --release` succeeds; service binary boots and serves `/healthz`. | PASS   |
| §4 Story 1 — validation rejects bad agent writes (SC-001) | `us1_validation::missing_name_userfact_is_422`, `hostname_shape_rejected_even_for_user_source`, `far_future_event_rejected` — 3/3 pass. | PASS   |
| §5 Story 2 — dissent on lower-trust contradiction (SC-002, SC-003) | `us2_dissents::{dissent_dedupe_path, dissent_lifecycle_promote, dissent_discard_marks_resolved}` — 3/3 pass. Promote/discard + 409 version_conflict covered. | PASS   |
| §6 Story 3 — decay-aware ranking (SC-004)               | `us3_decay::{task_fact_decays_faster_than_user_fact, search_orders_user_above_task_for_shared_term, tick_is_monotonically_non_increasing}` — 3/3 pass. | PASS   |
| §7 Story 4 — viewport curation flow (SC-005)            | `viewport/src-tauri`: 14 cargo tests pass (memory commands incl. promote/discard/upsert/edit/delete + 403/410/409 paths); SvelteKit `pnpm check` clean; `pnpm test` 21/21 pass. Live Windows GUI walk-through is out of scope for an automated check. | PASS (automated coverage) |
| §8 Story 5 — `just gate` is the constitution gate (SC-006, SC-007) | `just gate` runs fmt-check + clippy `-D warnings` + workspace tests and exits 0; `.github/workflows/ci.yml` service job invokes exactly `just gate`. | PASS   |
| FR-032 — no Phase 1 regression                          | All 001 integration tests (`us1_facts`, `us2_events`, `us3_knowledge`, `us4_unified_search`, `us5_health`, `perf_smoke --ignored`) re-run green after fixture payloads updated to satisfy Phase 2 schemas — no production-code regressions. | PASS   |

Notes on what the table cannot prove from a Linux CI host:
- The SC-005 acceptance scenario in §7 includes an interactive
  Windows viewport walkthrough (Edit → Delete → Dissents page).
  Coverage is automated end-to-end in the Tauri command tests and
  SvelteKit unit tests; a manual run on the Windows machine after
  `just viewport-build` is the final gate operators perform out of
  band.
- SC-007's "30 seconds on a warm host" is a perf claim; the
  light-mode `scripts/verify-mvp.sh --light` invoked by `just
  health` is the functional half (always-on `/healthz` + a fact
  round-trip).
