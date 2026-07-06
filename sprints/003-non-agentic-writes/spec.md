# Feature Specification: Non-Agentic Writes, Integrations, and the Systemd Switchover

**Feature Branch**: `003-non-agentic-writes`  
**Created**: 2026-05-18  
**Status**: Draft  
**Input**: User description: "We are ready to start on Phase 3 of `sprints/planning/plan.md`; to facilitate a practical test, let's add a deliverable of a handoff document for the ansible-k project `/home/ken/ansible-k` (we can either create it in this project and I can copy it over or we can create it directly in the ansible-k project, suggest creating directory `/home/ken/ansible-k/specs/klams-integration`)."

This sprint operationalizes Phase 3 of [the master plan](../planning/plan.md):
"Non-agentic writes and integrations." The system stops being a thing only
Ken (or the controller) writes to by hand and starts updating itself from
the homelab's existing automation surface — Ansible plays, a repo/notes
scanner, service monitors, and controller execution traces. It also flips
the klams service from the developer-loop `just run` workflow to a
managed `systemd` unit on `kubs0`, which is a prerequisite for Phase 6's
MCP server having something reliably running to talk to.

To make the Ansible integration **real instead of theoretical**, the sprint
also ships a stand-alone handoff document targeting the sibling
`ansible-k` project. The handoff lives at
`/home/ken/ansible-k/specs/klams-integration/` and is the contract the
ansible-k owner (Ken wearing his ops hat) uses to land the actual
callback/post-play hook plumbing in that repo. The klams sprint *writes
the contract and proves the receiving endpoints work*; the actual
play-side wiring is owned by ansible-k.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Ansible plays publish host facts into klams (Priority: P1)

When Ken runs an Ansible play that gathers or changes host state on a
homelab machine (`kubs0`, `kubsdb`, `kai`, future machines), the
relevant facts land in klams as `UserFact` / `EnvFact` rows automatically
— no manual `curl`, no separate "now go update memory" step. This is
the highest-priority integration because it eliminates the largest
remaining source of memory drift in the homelab (machines whose
documented state and actual state silently diverge).

**Why this priority**: Drift between "what `klams` says about a machine"
and "what Ansible actually configured on that machine" is exactly the
class of problem Phase 2 built the validation/dissent machinery for —
but the machinery is wasted if nothing is wiring trusted writes in. P1.

**Independent Test**: Run a representative ansible-k play (e.g.
`gather-gpu-facts.yml`) on `kubs0` from a controller machine with the
klams integration enabled. Without any manual follow-up, the resulting
GPU/hostname/package-version facts appear in
`GET /memory/facts?type=EnvFact&host=kubs0` with `source=Task` and a
non-empty `last_used_at`. Re-running the play without changing host
state results in **zero** new fact versions (canonical-hash dedupe
holds end-to-end).

**Acceptance Scenarios**:

1. **Given** the ansible-k handoff has been implemented in that repo and
   a play is run, **When** the play completes successfully, **Then**
   each fact the play emitted is present in klams with
   `source=Task`, the play's run-id in the payload, and the play's host
   in `host`.
2. **Given** a play that emits a fact identical to one already stored,
   **When** the play runs a second time, **Then** the fact's
   `version` is unchanged and only `last_used_at` advances.
3. **Given** a play running against an unreachable klams service,
   **When** the play completes, **Then** the play exits non-zero with
   a clear diagnostic, but the play's primary host changes are **not
   rolled back** (memory publishing is best-effort, not a gate on ops
   work).

---

### User Story 2 — Repo and notes scanner keeps knowledge memory current (Priority: P1)

A scheduled scanner (systemd timer on `kubs0`) walks Ken's source tree
(`~/src`) and his Obsidian vault (`~/obsidian`), chunks newly-changed
content, embeds it, and indexes it into `knowledge_items`. New notes
become searchable through `POST /memory/search` within one scan cycle
(default: 1 hour, configurable). Deleted files have their corresponding
chunks removed; modified files re-embed only the changed chunks.

**Why this priority**: The Phase 1 knowledge-memory path exists but is
empty unless someone hand-feeds it. The vault and source tree are
Ken's external long-term memory; until the scanner is running, the
"unified search" claim in the elevator pitch is aspirational. P1.

**Independent Test**: Add a new `.md` note to `~/obsidian/...` with a
unique nonce string. Within one scan cycle, `POST /memory/search` for
that nonce returns the chunk with the correct `file` and `repo`
metadata. Edit the note in place — within one scan cycle, search
returns the new text (not the old). Delete the note — within one scan
cycle, search returns no results for the nonce.

**Acceptance Scenarios**:

1. **Given** a fresh note in the vault and the scanner running on its
   normal interval, **When** the next scan completes, **Then** the
   note's chunks are present in `knowledge_items` with `source=Task`.
2. **Given** an existing note whose content changes between scans,
   **When** the next scan completes, **Then** only the affected
   chunks are re-embedded (verified via metric counter); unchanged
   chunks keep their original `created_at`.
3. **Given** the scanner crashes mid-walk, **When** it restarts,
   **Then** it resumes from a persisted cursor and does not re-embed
   files it already processed in the previous run.

---

### User Story 3 — Service monitors and execution traces flow into klams (Priority: P2)

Long-running homelab services (the existing systemd units on `kubs0`:
qdrant, postgres, tei, klams itself, future additions) emit lifecycle
events (`service.up`, `service.down`, `service.restart`,
`service.version_changed`) into `POST /memory/events`. Controller-driven
task execution traces — the per-task log of "agent X ran tool Y at time
Z" — also flow into `events`. Both use `source=Task` or
`source=Controller` and the existing Phase 1 event schema; no new
endpoint, just new producers.

**Why this priority**: This is what makes the events table actually
useful for "what changed on the homelab in the last 24 hours?" queries
and for post-incident review. P2 because the data is recoverable from
`journalctl` and controller logs if the producers are added later — but
the value compounds with time, so earlier is better.

**Independent Test**: Restart `qdrant` on `kubs0` via `systemctl
restart qdrant`. Within 30 seconds, `GET /memory/events?category=Service`
shows a `service.down` followed by a `service.up` event for
`qdrant`. Run a representative controller task that uses three tools;
the resulting `events` rows form a coherent trace with the same
`task_id`.

**Acceptance Scenarios**:

1. **Given** a monitored service restarts, **When** the next monitor
   poll fires, **Then** `events` contains a `service.down` and
   `service.up` event with matching `service` payload field.
2. **Given** a controller runs a task with N tool invocations,
   **When** the task ends, **Then** N execution-trace events share
   a `task_id` and their `created_at` values are monotonically
   non-decreasing.
3. **Given** the events table is queried for "last 24h on kubs0",
   **When** the query runs, **Then** results include both service
   lifecycle events and execution traces, ordered by `created_at`.

---

### User Story 4 — klams runs as a managed systemd unit on kubs0 (Priority: P1)

The day-to-day production workflow on `kubs0` is `systemctl
start/stop/restart/status klams-service`, not `just run` in a
detached tmux. The systemd unit pins the binary path, sets the
`KLAMS_CONFIG` environment variable, depends on `postgresql.service`
and `qdrant.service`, restarts on failure with sane backoff, and
streams logs to `journalctl`. `just run` survives as the **foreground
debugger** workflow (documented as such), but is no longer the path
Ken uses to bring the service up after a reboot.

**Why this priority**: Phase 6's MCP server (and, more immediately,
sprint 003 itself — the scanner timer, the monitors, the ansible-k
plays) all assume `klams-service` is reliably running and survives
reboots. P1 because it unblocks the rest of this sprint.

**Independent Test**: Reboot `kubs0`. After the reboot,
`systemctl is-active klams-service` returns `active` within 30 seconds
of postgres/qdrant being up, with no manual intervention. Kill the
process (`systemctl kill --signal=SIGKILL klams-service`); systemd
restarts it within the configured backoff. `journalctl -u
klams-service` shows structured logs identical to what `just run`
produces.

**Acceptance Scenarios**:

1. **Given** the systemd unit is installed, **When** `kubs0`
   reboots, **Then** klams comes up automatically and `/healthz`
   returns 200 within 30 seconds of postgres/qdrant being ready.
2. **Given** klams is running under systemd, **When** the process
   is killed, **Then** systemd restarts it within the configured
   backoff and `/healthz` recovers without operator action.
3. **Given** the developer wants to debug a panic, **When** they run
   `just run` after stopping the unit, **Then** the foreground
   binary serves traffic on the same port with the same config
   semantics as the unit (no divergence between "prod" and "dev"
   modes).

---

### User Story 5 — Per-source write policy is enforced and visible (Priority: P2)

Phase 2 introduced the trust hierarchy (`User > Controller > Task >
AgentProposal`) and the dissents diversion path for lower-trust
contradictions. Phase 3 closes the loop by enforcing that policy at
the API boundary: trusted sources (`User`, `Controller`, `Task`) write
directly to canonical storage; only `AgentProposal` writes that
contradict a higher-trust source are diverted to `dissents`. The
behavior must be **inspectable** — a single response field plus a
metric counter tells the writer which path their request took.

**Why this priority**: P2 because the Phase 2 code already enforces
most of this; this sprint hardens the API contract and the metrics so
the new non-agentic writers (Ansible, scanner, monitors) get
predictable, debuggable behavior from day one.

**Independent Test**: Submit two `UserFact` payloads against the same
key — first with `source=AgentProposal`, then with `source=User`
contradicting the first. The first lands canonical (no prior fact);
the second lands canonical and bumps version (User outranks
AgentProposal). Then submit a third contradicting `AgentProposal`:
it lands in `dissents`, not canonical. The metric
`klams_writes_total{path="dissent"}` increments exactly once across
these three requests.

**Acceptance Scenarios**:

1. **Given** a canonical fact with `source=User` exists,
   **When** an `AgentProposal` submits a contradicting payload,
   **Then** the response includes `path: "dissent"` and a
   `dissent_id`, and `klams_writes_total{path="dissent"}` increments.
2. **Given** a canonical fact with `source=AgentProposal` exists,
   **When** a `User` source submits a contradicting payload,
   **Then** the response includes `path: "canonical"`, the canonical
   row's `version` is bumped, and `source` is updated to `User`.
3. **Given** the policy table is exposed via `GET /memory/policy`
   (read-only), **When** an integrator fetches it, **Then** the
   returned table matches what the service actually enforces.

---

### User Story 6 — Handoff document is the contract for ansible-k (Priority: P1)

A self-contained handoff document at
`/home/ken/ansible-k/specs/klams-integration/` (created in this sprint
and committed in the ansible-k repo by Ken) gives the ansible-k owner
everything needed to land User Story 1's play-side wiring **without
needing to read any klams source code**. It is the API contract, the
auth model, the suggested implementation shape (callback plugin vs
post-play hook), the failure-mode expectations, and the test
recipe — packaged as something the receiving project can drop into
its own `sprints/` directory and run a normal speckit cycle against.

**Why this priority**: Without the handoff, User Story 1 is a klams
deliverable masquerading as an ansible-k deliverable. P1 because it
is the only thing in this sprint that crosses a project boundary, and
the cross-boundary contract is exactly the kind of thing that rots
silently if not written down.

**Independent Test**: Open `/home/ken/ansible-k/specs/klams-integration/`
on a machine that has never read the klams source. From the document
alone, hand-construct a `curl` (or short Python `aiohttp` snippet)
that successfully posts a `UserFact` to klams with `source=Task` and
gets a 200 back; the response shape and the auth flow match what the
document promised. Re-read the document six months later and confirm
the API contract it describes is still accurate (or, if not, the
document points at the version of klams it was written against).

**Acceptance Scenarios**:

1. **Given** the handoff document exists, **When** the ansible-k
   owner reads it cold, **Then** they can name (a) the endpoint,
   (b) the auth header, (c) the minimal valid `UserFact` payload
   shape, (d) the expected dedupe behavior, and (e) the failure
   modes (transient 5xx vs validation 422 vs version 409) without
   reading klams source.
2. **Given** the handoff specifies a fact payload shape,
   **When** that shape is sent to a sprint-003 klams instance,
   **Then** the request succeeds and the resulting fact passes the
   Phase 2 validators (no schema surprises).
3. **Given** the ansible-k owner opens a follow-up speckit cycle
   against the handoff, **When** they generate `tasks.md` from it,
   **Then** the tasks are concrete (write callback plugin X, add
   inventory var Y, register systemd timer Z) — not "figure out the
   integration."

### Edge Cases

- **klams is down when an Ansible play runs**: play must complete its
  primary work, log a clear "memory publish skipped/failed" diagnostic,
  and exit non-zero only if `klams_required: true` is set in the play
  variables (default: false).
- **Scanner encounters a binary or oversize file**: skip with a
  one-line `tracing::warn` log; do not block the rest of the scan;
  record a metric counter `klams_scanner_skipped_total{reason=...}`.
- **Scanner encounters a file it has indexed before but whose
  embedding model has changed**: re-embed unconditionally; bump a
  separate metric counter so the operator can correlate cost spikes
  with model swaps.
- **Service monitor produces duplicate `service.up` events** (e.g.
  the monitor itself restarts): events are append-only by design;
  consumers must tolerate duplicates. Dedupe is not in scope.
- **systemd unit references a binary path that no longer exists**
  (e.g. after a botched `just build`): unit fails to start with a
  clear `journalctl` error; previous good binary is preserved by a
  Phase-3-provided `klams-service` / `klams-service.prev` rotation
  step in the deploy recipe.
- **ansible-k repo does not yet exist on the machine reading the
  handoff**: the handoff document is self-contained markdown; it
  does not assume the reader has cloned ansible-k.

## Requirements *(mandatory)*

### Functional Requirements

**Ansible integration (US1)**

- **FR-001**: System MUST accept `POST /memory/facts` requests carrying
  Ansible-originated facts with `source=Task` and treat them as
  trusted (canonical path; no dissent diversion unless they contradict
  a higher-trust `User` row).
- **FR-002**: System MUST validate the `task_id` payload field whenever it
  is present on a `source=Task` request (Ansible run-id shape per
  [data-model.md §1](data-model.md): UUID or `ansible-`-prefixed run-id,
  length ≤ 64) and MUST reject malformed values with
  `422 payload.task_id invalid`. A `source=Task` write WITHOUT `task_id`
  remains accepted (existing Phase 1 controller traces); the Ansible-vs-
  controller distinction is by payload shape, not a separate enum value.
- **FR-003**: System MUST canonically-hash Ansible-originated facts so
  that re-running an unchanged play produces zero new versions (only
  `last_used_at` advances).

**Scanner (US2)**

- **FR-004**: System MUST ship a scanner binary (or a
  `klams-service`-internal periodic task — implementation choice
  deferred to plan.md) that walks configured roots (default:
  `~/src`, `~/obsidian`) on a configurable interval (default: 3600s).
- **FR-005**: The scanner MUST persist a cursor (file path + content
  hash + mtime) so that incremental scans re-embed only changed files.
- **FR-006**: The scanner MUST honor a `.klamsignore` file (gitignore
  syntax) in each walked root, and MUST always skip the standard
  build/output directories (`target/`, `node_modules/`, `.git/`,
  `__pycache__/`, `.venv/`).
- **FR-007**: The scanner MUST emit Prometheus metrics for
  `klams_scanner_files_processed_total`,
  `klams_scanner_files_skipped_total{reason}`,
  `klams_scanner_chunks_indexed_total`, and
  `klams_scanner_last_run_timestamp_seconds`.
- **FR-008**: When a file is deleted between scans, the scanner MUST
  delete the corresponding `knowledge_items` chunks (by
  `source_file` payload field) within one scan cycle.

**Service monitors and traces (US3)**

- **FR-009**: System MUST accept `POST /memory/events` requests with
  `category=Service` and a constrained payload (`service`, `event`
  ∈ {`up`,`down`,`restart`,`version_changed`}, `host` required,
  `version` optional, `port` optional). `host` enables multi-host
  monitor deployments (FR-011) to distinguish same-named units across
  machines.
- **FR-010**: System MUST accept `POST /memory/events` requests with
  `category=Execution` and a `task_id`, and MUST index `events` on
  `(task_id, created_at)` so per-task trace queries are O(log n).
- **FR-011**: The Phase 3 deploy MUST ship a small `klams-monitor`
  systemd unit (or equivalent) on `kubs0` that polls `systemctl
  is-active` for a configurable list of unit names and posts
  `service.*` events on state changes.

**Systemd switchover (US4)**

- **FR-012**: System MUST ship a `deploy/klams-service.service`
  systemd unit file that depends on `postgresql.service` and
  `qdrant.service`, restarts on failure (`Restart=on-failure`,
  `RestartSec=5`), runs as a non-root user, and reads its config
  from `/etc/klams/klams.toml` (or `KLAMS_CONFIG` env override).
- **FR-013**: System MUST ship a `just` recipe (or shell script) that
  installs the unit, reloads systemd, enables the unit, and starts
  it — idempotently.
- **FR-014**: System MUST ship a deploy recipe that rotates the
  installed binary (`klams-service` → `klams-service.prev`) before
  copying the new one, so a failed start can be reverted with one
  command.
- **FR-015**: Documentation MUST clearly call out that `just run`
  is the foreground-debugger workflow only; production lifecycle is
  via `systemctl`.

**Per-source policy (US5)**

- **FR-016**: System MUST include a `path` field
  (`"canonical"` | `"dissent"`) and, when path is `"dissent"`, a
  `dissent_id` in every write-endpoint response.
- **FR-017**: System MUST expose a metric
  `klams_writes_total{type, source, path}` covering every write.
- **FR-018**: System MUST expose a read-only `GET /memory/policy`
  endpoint that returns the source-trust table as JSON, derived from
  the same data structure the dispatcher uses (no hand-maintained
  duplication).

**Handoff document (US6)**

- **FR-019**: This sprint MUST produce a self-contained handoff
  document at `/home/ken/ansible-k/specs/klams-integration/` (with
  at minimum `README.md`, `spec.md`, and `api-contract.md`) that
  contains: endpoint list, auth model, minimal valid payload examples
  for `UserFact` / `EnvFact` / `Event`, dedupe semantics, failure
  modes (422/409/5xx with response shapes), suggested integration
  shape (callback plugin vs post-play hook with trade-offs), test
  recipe (a `curl` walkthrough the ansible-k owner can run against a
  klams test stack), and an explicit "this document is pinned to
  klams sprint-003 API surface" header.
- **FR-020**: The handoff document MUST NOT require the reader to
  open any klams source file; every piece of information they need
  to wire up an integration MUST be in the document itself (or
  reachable via a link to a stable klams doc, not a source file).
- **FR-021**: The handoff document MUST include a
  "how to detect drift" section: when a future klams release breaks
  the contract, how does the ansible-k owner notice (suggested:
  pin the klams `/healthz?contract=v1` response, fail noisily if
  absent).

**Cross-cutting (constitution)**

- **FR-022**: `just gate` (defined in sprint 002) MUST remain green
  for every commit on the `003-non-agentic-writes` branch; no new
  clippy or fmt drift introduced by this sprint.
- **FR-023**: All sprint-001 and sprint-002 integration tests MUST
  continue to pass; no regressions in the Phase 1 or Phase 2
  contract suite (`us1_*`, `us2_*`, `us3_*`, `us4_*`, `us5_*`).

### Key Entities

- **AnsibleFactWrite**: An Ansible-originated `UserFact` or `EnvFact`,
  carrying `task_id` (Ansible play run id), `host`, and the standard
  fact payload. Distinguished from other Task-source writes only by
  the presence of `task_id` matching the Ansible run-id format.
- **ScannedKnowledgeChunk**: A chunk produced by the scanner, with
  payload fields `source_file` (absolute path), `repo` (the top-level
  directory under a configured root), `chunk_index`, `content_hash`,
  and the standard knowledge-item metadata.
- **ServiceEvent**: An event with `category=Service`, payload fields
  `service`, `event` (enum), `version`, `port`. The Phase 1 event
  envelope is unchanged; this is a constrained payload shape.
- **ExecutionTraceEvent**: An event with `category=Execution`,
  payload fields `task_id`, `tool`, `phase`
  (`started`|`completed`|`failed`), and free-form `detail`. Indexed
  on `(task_id, created_at)`.
- **PolicyTable**: The read-only JSON projection of the source-trust
  table. Shape: `{ "User": { "rank": 4, ... }, "Controller": {
  "rank": 3, ... }, "Task": { "rank": 2, ... }, "AgentProposal": {
  "rank": 1, ... } }` (exact rank scheme deferred to plan.md).
- **HandoffDocument**: A directory tree under
  `/home/ken/ansible-k/specs/klams-integration/` containing
  speckit-compatible `README.md`, `spec.md`, `api-contract.md`,
  and a `curl` walkthrough script. Pinned to klams sprint-003 API
  surface by an explicit version header.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: When a representative Ansible play (e.g.
  `gather-gpu-facts.yml` against `kubs0`) is run twice in a row from
  a controller machine with the integration enabled, the first run
  produces N new `EnvFact` versions and the second run produces
  **zero** new versions; `last_used_at` advances in both runs.
- **SC-002**: A new note added to `~/obsidian/` is returned by
  `POST /memory/search` within one scan cycle (≤ 1 hour at default
  interval; ≤ 60 seconds when the scanner is triggered manually for
  the test).
- **SC-003**: After `systemctl restart qdrant` on `kubs0`, a
  `GET /memory/events?category=Service&service=qdrant&since=-2m`
  returns at least one `service.down` and one `service.up` event,
  in that order, within 30 seconds of the restart command returning.
- **SC-004**: After a clean `kubs0` reboot, `systemctl is-active
  klams-service` returns `active` and `/healthz` returns 200 within
  30 seconds of postgres+qdrant being ready, with no manual
  intervention. The previous-good binary rotation step has been
  exercised at least once (a deliberate-bad-build rollback test).
- **SC-005**: `GET /memory/policy` returns a JSON table that
  matches the source-trust hierarchy in §6 of
  [plan.md](../planning/plan.md) and is derived from the same Rust
  data structure the write dispatcher uses (verified by a unit test
  that compares the served JSON against the in-memory struct).
- **SC-006**: An engineer with no klams source-code access can, using
  only `/home/ken/ansible-k/specs/klams-integration/` and a klams
  test stack, post a valid `UserFact` and receive a 200 within
  10 minutes of opening the document for the first time.
- **SC-007**: `just gate` runs in CI on every PR commit against this
  branch and stays green; the sprint-001 and sprint-002 integration
  suites continue to pass with zero regressions.

## Assumptions

- Phase 1 (`sprints/001-initial-mvp/`) and Phase 2
  (`sprints/002-safety-and-write-ops/`) are shipped and live on `kubs0`
  via the production compose stack (the binary may still be pre-Phase
  2 on the live :7777 endpoint at sprint-003 kickoff; a build+restart
  of the existing service is the first deploy step of this sprint).
- The `ansible-k` project at `/home/ken/ansible-k` exists and is
  Ken's; the handoff document is a contract from klams-the-project to
  ansible-k-the-project, both owned by Ken. There is no third-party
  coordination cost.
- The Obsidian vault path is `~/obsidian` on `kubs0`; the source-tree
  root is `~/src`. Both are direct filesystem reads; no network
  fetches.
- `kubs0` is the production target. The scanner, monitor, and
  systemd unit are installed on `kubs0` only. Other homelab machines
  are clients, not hosts, for these services.
- The embedding model and Qdrant collection schema are unchanged from
  Phase 1; the scanner produces chunks in the same format the Phase 1
  `POST /memory/knowledge/index` endpoint accepts (it calls that
  endpoint rather than writing to Qdrant directly).
- The systemd switchover does not break the existing developer
  workflow on a non-`kubs0` dev box (`just run` continues to work
  exactly as before; this sprint adds a deployment path without
  removing the dev path).
- The handoff document is delivered as a directory under
  `/home/ken/ansible-k/specs/klams-integration/` (created by this
  sprint with Ken's explicit go-ahead). If Ken prefers it staged
  inside this klams repo first, the same files can be authored under
  `sprints/003-non-agentic-writes/handoff/` and `cp -r`'d over; the
  spec's acceptance criteria target the final location either way.
- Auth model for non-agentic writers is the existing bearer token from
  the klams config file; per-source tokens (one for the scanner, one
  for the monitor, one for ansible-k) are out of scope for this
  sprint and tracked separately (Phase 6 / backlog).
- Items not in this sprint per plan.md: hybrid retrieval,
  summarization, `/memory/context`, backups, dashboards,
  `maintenance_mode`, and the MCP server — all Phase 4+.

## Phase 3 walkthrough

Per task T057 and following the sprint-002 pattern (see
`sprints/002-safety-and-write-ops/spec.md` § "Phase 2 walkthrough"),
the [quickstart.md](quickstart.md) flow §1–§8 was walked against
the sprint-003 build. Each row uses the closest mechanical proxy
that the implementation host can produce without rebooting
`kubs0` mid-sprint; the live systemd/reboot rows fall back to
the staged unit files and the `install-systemd.sh --dry-run`
harness.

The integration suite ran against
[`tests/docker-compose.test.yml`](../../tests/docker-compose.test.yml)
with `TEST_DATABASE_URL`, `TEST_QDRANT_URL`, and `TEST_TEI_URL`
pointing at the test stack — identical to what CI exercises.

| Step | Evidence | Result |
|------|----------|--------|
| §1 — build + deploy the sprint-003 binaries | `cargo build --release --bin klams-service --bin klams-scanner --bin klams-monitor` succeeds; `deploy/install-systemd.sh --dry-run` enumerates all four units; `tests/install_systemd_dry_run.rs` (1/1) + `tests/deploy_unit_files.rs` (4/4) pass. | PASS (dry-run + harness) |
| §2 — `GET /memory/policy` (US5)             | `us3a_policy_endpoint::*` 3/3 pass — bearer required, policy returns JSON with `decay`, `dedupe`, `dissent_thresholds`. | PASS |
| §3 — `path` field on write responses (US1 prereq) | `contract_facts::post_facts_returns_persisted_fact_shape` (asserts `path: "canonical"` on the flattened `Fact` shape) + the parallel contract checks in `contract_events` / `contract_knowledge`; `us2_dissents::{dissent_lifecycle_promote, dissent_dedupe_path, dissent_discard_marks_resolved}` exercise the `path: "dissent"` divert + promote round-trip — 3/3 pass. | PASS |
| §4 — scanner indexes a fresh note within one cycle (US2, SC-002) | `us3d_scanner_e2e::fresh_file_is_indexed_edit_replaces_delete_removes` (1/1) runs the full walk → chunk → `POST /memory/knowledge/index` pipeline against a temp tree and asserts qdrant returns the chunk + that edits replace and deletes remove; `klams-scanner` lib tests cover cursor persistence + skip-on-unchanged. | PASS |
| §5 — monitor emits `service.*` events on restart (US3, SC-003) | `us3c_events::*` (2/2) cover edge-transition emission; `klams-monitor` lib tests (3/3) cover poll/diff/post. | PASS |
| §6 — reboot resilience (US4, SC-004)        | All four unit files (`klams-service.service`, `klams-scanner.service`, `klams-scanner.timer`, `klams-monitor.service`) declare correct `After=`/`Wants=`/`Restart=` directives, verified by `tests/deploy_unit_files.rs` parsing the staged files. Live `systemctl is-active` post-reboot is the manual operator gate after `just install-systemd` lands on `kubs0`. | PASS (unit-file shape verified; live reboot is operator gate) |
| §7 — sprint-002 walkthrough still passes (FR-023) | `cargo test -p klams-service --tests --all-features -- --include-ignored --skip search_p95 --test-threads=1` runs 54 integration tests across `us1_facts`, `us2_dissents`, `us2_events`, `us3_decay`, `us3a_policy_endpoint`, `us3b_ansible_facts`, `us3c_events`, `us3d_scanner_e2e`, `us3e_handoff_layout`, `us4_unified_search`, `us5_health`, plus contract suites — 100% pass, zero regressions. `just gate` (fmt + clippy `-D warnings` + workspace tests) exits 0. | PASS |
| §8 — handoff shipped (SC-006)               | `sprints/003-non-agentic-writes/handoff/` contains `README.md`, `spec.md`, `api-contract.md`, and `examples/post-userfact.sh` (POSIX `sh`, +x). `cp -r`'d to `/home/ken/ansible-k/specs/klams-integration/`; `us3e_handoff_layout` (4/4) asserts the layout invariants. | PASS |

Notes on what the table cannot prove from the implementation host:
- §6 ("reboot resilience") is fully exercised only by a live
  reboot of `kubs0` after `just install-systemd`. The unit-file
  shape (the only artifact this sprint owns) is verified by the
  Rust harness; the actual `systemctl is-active` check is the
  operator's post-deploy step.
- §1's "deploy" half runs under `--dry-run` here so this machine
  doesn't mutate `/etc/systemd/system` mid-sprint; the
  install path runs end-to-end on `kubs0` when Ken triggers
  `just install-systemd`.
