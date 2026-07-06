# Feature Specification: Stability & Attribution

**Feature Branch**: `009-stability-attribution`
**Created**: 2026-05-27
**Status**: Draft
**Input**: User description: "Sprint 009 — close known defects (klams-service loopback CLOSE_WAIT leak from kwi #26, REST-write author attribution data-integrity bug, viewport Authors→memory 404 from kwi #28), repair existing data via one-shot re-attribution migration, refresh the perf baseline that was deferred while #26 was open, and tighten Phase 6 test isolation."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - klams-service stays reachable under sustained loopback traffic (Priority: P1)

A homelab operator runs `klams-service` continuously on `kubs0`. Local
clients (the viewport's dev loop, healthchecks, ad-hoc `curl` calls)
open connections to `127.0.0.1:7777` throughout the day, and some of
those clients exit without cleanly closing their sockets. Today the
service eventually accumulates so many `CLOSE_WAIT` sockets that it
exhausts its file-descriptor cap and stops accepting new connections —
`/healthz` times out and the operator must restart the process with a
raised `ulimit -n`. After this sprint, the service must survive the
same traffic pattern indefinitely without operator intervention.

**Why this priority**: When the service wedges, every klams client
loses access to memory — viewport, MCP, REST, bench, perf rerun.
Nothing else in this sprint can be safely measured or validated until
the service is reliable. This is also the sole blocker recorded as an
open bug (kwi #26).

**Independent Test**: Run a loopback soak that opens a connection,
sends a partial HTTP request, then closes the client side without
reading the response — at a steady rate for an extended period.
Observe the service's file-descriptor count and `:7777` socket-state
histogram. The fd count and `CLOSE_WAIT` count must remain bounded
(no monotonic growth) for the duration of the soak, and `/healthz`
must continue to return 200 throughout.

**Acceptance Scenarios**:

1. **Given** klams-service is running with default operational limits,
   **When** a loopback client opens a connection, writes a partial
   request, and exits without reading the response,
   **Then** the server must reclaim the socket within a bounded
   timeout (rather than holding it in `CLOSE_WAIT` until the process
   exits).
2. **Given** klams-service has been running under sustained half-close
   traffic for the duration of a soak run,
   **When** the operator queries `/healthz`,
   **Then** the endpoint must return 200 and the process file-
   descriptor count must be within a configurable cap.
3. **Given** klams-service is managed by a systemd unit on a packaged
   deployment,
   **When** the service starts,
   **Then** its file-descriptor cap must be raised from the host
   default to a level that supports the documented connection load,
   without requiring the operator to set `ulimit -n` manually.

---

### User Story 2 - REST-written memories are correctly attributed to the writing agent (Priority: P1)

The klams Activity tab and per-author memory listings exist so an
operator can answer questions like "what did the controller record
overnight?" or "which facts did the ansible-k agent push?". Today
every memory written through the REST surface (`POST /v1/facts`,
`POST /v1/events`, `POST /v1/knowledge`) is attributed to the seeded
`system` author regardless of which bearer token issued the write. The
result is that the Activity tab over-reports `system` and under-reports
every real agent; per-author aggregates and filters are likewise wrong.
After this sprint, a REST write must appear in the store and in the
Activity tab under the actual writing agent's name.

**Why this priority**: This is a data-integrity bug. Every per-author
surface shipped in sprint 008 — the Activity tab, author summary
aggregates, `GET /v1/authors/{id}/memories`, the audit story —
silently lies about who wrote what. It also blocks the perf rerun
(Story 5) because a fresh smoke run would re-pollute the store with
60k more `system`-stamped rows.

**Independent Test**: Configure two bearer tokens, each mapped to a
distinct named agent. Write one fact through each token. Query
`GET /v1/authors/{id}/memories` for each agent and confirm that the
fact written by token A appears only under agent A, and the fact
written by token B appears only under agent B — neither is attributed
to `system`.

**Acceptance Scenarios**:

1. **Given** a bearer token is configured with an agent name,
   **When** the operator sends a REST write request (`POST /v1/facts`,
   `POST /v1/events`, or `POST /v1/knowledge`) using that token,
   **Then** the resulting row in the store must be attributed to the
   author registered for that token's agent name — not to `system`.
2. **Given** two bearer tokens are configured with two different
   agent names,
   **When** each token writes a memory,
   **Then** the Activity tab and per-author listings must show each
   memory under its writing agent, with no cross-attribution and no
   `system` attribution.
3. **Given** a bearer token has no agent name configured,
   **When** that token writes a memory,
   **Then** the system must continue to accept the write (existing
   deployments stay valid) and attribute it to a documented default
   author.
4. **Given** the bench seeder writes its corpus through a configured
   `klams-bench` token,
   **When** the operator inspects the store after a seed run,
   **Then** every seeded row must be attributable to the `klams-bench`
   author (not `system`), and `just bench-clean` must purge them via
   that author attribution alone.

---

### User Story 3 - Historical REST writes are re-attributed to the correct agent (Priority: P1)

Even after Story 2 lands, the rows already in the live store will
remain stamped `system`. The Activity tab and per-author aggregates
will look correct for new writes but will continue to mis-report
historical writes until the existing data is repaired. After this
sprint, a one-shot repair must walk the existing data, recover the
true author for each `system`-stamped row where the provenance is
unambiguous, and update the attribution in place.

**Why this priority**: Without this, every historical view is wrong
forever — operators have to mentally subtract a poorly-defined slice
of `system` activity from every aggregate. The repair must run once
(idempotent reruns are safe but no-ops). It is P1 because it ships
alongside the Story 2 fix that creates the dividing line.

**Independent Test**: Take a snapshot of the per-author memory counts
before the repair. Run the repair. Re-query the counts and confirm
that (a) the total memory count is unchanged, (b) the number of rows
attributed to `system` has dropped, (c) the rows that moved off
`system` are attributed to authors whose existence in the store
predates this sprint, and (d) any row left under `system` corresponds
to a write whose original provenance was genuinely the system author
or could not be unambiguously recovered.

**Acceptance Scenarios**:

1. **Given** the live store contains memories stamped with the
   `system` author whose true writing agent can be unambiguously
   recovered from existing provenance,
   **When** the operator runs the one-shot re-attribution repair,
   **Then** those memories must be re-attributed to their true
   writing agent and the total memory count must be unchanged.
2. **Given** the live store contains memories whose true writing
   agent cannot be unambiguously recovered,
   **When** the repair runs,
   **Then** those memories must remain attributed to `system` and
   the count of unrecoverable rows must be reported to the operator.
3. **Given** the repair has already run successfully on a store,
   **When** the operator runs it again,
   **Then** the repair must complete without error and report zero
   additional rows changed.

---

### User Story 4 - Operator can browse memories from the Authors view without hitting a 404 (Priority: P2)

In the viewport today, the path **Authors → Search → Select Author →
(optional) Show More** lists the author's memories. Clicking any item
in the **Summary** column navigates to a 404 page instead of opening
that memory's details pane. This breaks the operator's primary path
for inspecting an individual memory after locating it through the
author view. After this sprint, the Summary click must open the
details pane — the same pane reachable from the Activity tab and
other memory list surfaces.

**Why this priority**: Real annoyance for the operator but no data
loss and no service impact. It's a small, mechanical fix that fits
naturally alongside the attribution work that makes the Authors view
useful in the first place. Tracked as kwi #28.

**Independent Test**: With the viewport running against a store that
contains at least one author with at least one memory, navigate to
**Authors**, search for that author, select them, and click any
Summary cell in the memory list. The viewport must show the memory
details pane for that row, not a 404 page.

**Acceptance Scenarios**:

1. **Given** the viewport is showing the Authors view for an author
   with memories,
   **When** the operator clicks a row in the Summary column,
   **Then** the viewport must navigate to the details pane for that
   memory (not a 404).
2. **Given** the same memory can be reached from both the Activity
   tab and the Authors view,
   **When** the operator clicks it from either entry point,
   **Then** both clicks must resolve to the same memory details pane.

---

### User Story 5 - Refreshed full-corpus performance baseline (Priority: P2)

Sprint 008 had to ship a smoke-sized perf baseline (500 facts / 2,000
knowledge items) because running the full 10k/50k contract corpus
would have re-triggered the kwi #26 wedge or polluted the store
irreversibly. After Stories 1 and 2 land, the operator must be able
to run the documented full-corpus baseline end-to-end, attributable
to the `klams-bench` author, and commit a refreshed baseline that
honors the `100 samples across 10 queries` target documented in
`contracts/bench-harness.md`.

**Why this priority**: Documentation accuracy and a future
regression baseline. Not a defect users see directly; depends on
Stories 1 and 2.

**Independent Test**: Seed the contract corpus, run the harness,
inspect the refreshed `perf-baseline.md` against the published
target. Run `just bench-clean` and confirm the store returns to the
baseline counts that preceded the run.

**Acceptance Scenarios**:

1. **Given** Stories 1 and 2 are landed and the bench seeder is
   configured to write as `klams-bench`,
   **When** the operator runs the full-corpus seed and harness,
   **Then** the run must complete without service degradation and
   produce a refreshed `sprints/008-activity-observability/perf-baseline.md`
   matching the documented sample target.
2. **Given** a full-corpus seed has populated the store,
   **When** the operator runs `just bench-clean`,
   **Then** every seeded row must be removed via the `klams-bench`
   attribution alone, and the store must return to its
   pre-seed counts.

---

### User Story 6 - Phase 6 MCP tests can run together without skewed assertions (Priority: P3)

`crates/klams-service/tests/mcp_phase6.rs` currently shares a single
Qdrant collection and Postgres test database across tests. Running
them with `--test-threads=1` lets earlier tests leave rows that skew
counter assertions in `memory_admin_list_deleted_smoke`. After this
sprint, that test must be insulated from sibling test state so the
suite can run cleanly without ordering hacks.

**Why this priority**: Hygiene only — the underlying delete/restore
behavior is proven by sibling tests and by the live quickstart §8
walk. It costs little to bundle with the rest of the bug work.

**Independent Test**: Run the Phase 6 MCP test file (and only that
file) repeatedly under default parallelism. All cases must pass
without `--test-threads=1` and without prior-run cleanup.

**Acceptance Scenarios**:

1. **Given** the Phase 6 MCP test file,
   **When** the suite runs under default cargo test parallelism with
   no manual cleanup,
   **Then** every test must pass, including
   `memory_admin_list_deleted_smoke`, on every invocation.

---

### Edge Cases

- A token is reconfigured to a different agent name between service
  restarts — historical attribution under the old agent must be
  preserved; only new writes pick up the new name.
- The configured agent name for a token contains characters that
  the existing author validation rejects — the service must fail
  startup with a clear error rather than fall back to `system`.
- The one-shot re-attribution repair encounters provenance that
  resolves to an author who no longer exists, or provenance that is
  missing/ambiguous — the row must be reassigned to a documented
  `lost-author` identity (distinct from `system`) and counted in
  the unrecoverable bucket of the repair report.
- A loopback client opens a connection but never sends any bytes —
  the service must time it out at the connection layer.
- A REST write request includes an explicit author hint in its body
  — that hint must be ignored; attribution is bound to the bearer
  token, not to request content.

## Requirements *(mandatory)*

### Functional Requirements

**Service stability (Story 1)**

- **FR-001**: The service MUST reclaim sockets stuck in `CLOSE_WAIT`
  on the loopback listener within a bounded, configurable timeout —
  open connections from peers that vanish must not pin file
  descriptors indefinitely.
- **FR-002**: The service MUST enforce an upper bound on simultaneous
  open connections per remote peer so a misbehaving client cannot
  exhaust the file-descriptor cap on its own.
- **FR-003**: The service MUST continue to accept new connections and
  respond to `/healthz` with 200 throughout a soak run that exercises
  the conditions in FR-001.
- **FR-004**: A documented soak harness MUST exist that reproduces
  the loopback CLOSE_WAIT pattern and can be used to validate FR-001
  and FR-003.
- **FR-005**: The packaged systemd deployment MUST raise the service's
  file-descriptor cap above the host default without requiring the
  operator to set `ulimit -n` manually.

**Attribution wiring (Story 2)**

- **FR-006**: Each configured bearer token MUST carry an associated
  agent name, with a documented default applied when no name is
  configured.
- **FR-007**: At service startup, each token's agent name MUST be
  resolved to a registered author identity, creating the registration
  if it does not already exist.
- **FR-008**: Every REST write (`POST /v1/facts`,
  `POST /v1/events`, `POST /v1/knowledge`) MUST attribute the
  resulting row to the author bound to the bearer token, not to
  the seeded `system` author.
- **FR-009**: Every per-author surface (Activity tab, per-author
  listings, author aggregates) MUST reflect the bearer-bound author
  for memories written after this sprint lands.
- **FR-010**: An explicit author hint contained in a REST write
  request body MUST NOT influence attribution — only the bearer
  binding controls who is recorded as the writer.
- **FR-011**: The bench seeder MUST write its corpus as a dedicated
  `klams-bench` agent, and `just bench-clean` MUST purge the corpus
  using that author attribution alone (no payload-pattern fallback).
- **FR-012**: When a token's configured agent name is invalid, the
  service MUST refuse to start with a clear error rather than silently
  attributing its writes to a fallback author.

**Re-attribution repair (Story 3)**

- **FR-013**: A one-shot re-attribution repair MUST be available that
  walks the existing store and reassigns the writing author of
  `system`-stamped rows whose true author can be unambiguously
  recovered from existing provenance.
- **FR-014**: The repair MUST be idempotent — a second run on the
  same store must report zero additional rows changed and exit
  cleanly.
- **FR-015**: The repair MUST report counts of (a) rows reassigned
  to a recovered non-system author, (b) rows reassigned to the
  `lost-author` identity because their original author could not be
  unambiguously recovered or no longer exists, and (c) rows left
  under `system` because their original attribution was genuinely
  the seeded system author.
- **FR-016**: The repair MUST leave the total memory count unchanged
  — it reassigns attribution, never adds or removes rows. A second
  run on the same store MUST report the same total count and zero
  additional reassignments.
- **FR-016a**: A `lost-author` identity MUST be seeded (analogous to
  `system`) so the repair has a stable, queryable destination for
  rows whose true writer cannot be recovered. Per-author surfaces
  MUST treat `lost-author` like any other author for display and
  filtering.

**Viewport Authors view (Story 4)**

- **FR-017**: Clicking a memory's Summary cell from the Authors view
  MUST navigate to the same memory details pane reachable from other
  list surfaces.
- **FR-018**: The href computation used by the Authors view MUST be
  unified with the one used by the Activity tab so the two surfaces
  cannot drift apart again.

**Perf baseline refresh (Story 5)**

- **FR-019**: A refreshed full-corpus perf baseline MUST be committed
  at `sprints/008-activity-observability/perf-baseline.md`, matching
  the sample target documented in
  `sprints/008-activity-observability/contracts/bench-harness.md`.
- **FR-020**: Running and cleaning the full-corpus baseline MUST
  leave the live store at its pre-run baseline counts when
  `just bench-clean` completes.

**Test isolation (Story 6)**

- **FR-021**: The Phase 6 MCP test file MUST pass under default
  cargo test parallelism, with no reliance on `--test-threads=1` or
  on cleanup from prior runs.

### Key Entities

- **Bearer token grant**: A configured credential entry that maps a
  bearer secret to a permission scope and (after this sprint) to an
  agent name used for attribution of writes the token performs.
- **Author**: A registered identity that writes memories. Already
  exists in the store; this sprint extends the relationship so REST
  writes resolve to a real author instead of `system`.
- **Memory write provenance**: Existing per-write metadata (events,
  source attribution) that the one-shot repair walks to recover the
  true author of historical `system`-stamped rows.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A loopback half-close soak running for at least
  18 hours (chosen to fit within an overnight off-hours window so it
  doesn't interfere with day-time development against `klams`)
  leaves the service's open-file-descriptor count and `:7777`
  `CLOSE_WAIT` count at or below the count observed at the start of
  the soak.
- **SC-002**: kwi #26 is closed and the documented recovery steps
  ("`ulimit -n 65536` and restart") are removed from operational
  docs because they are no longer required.
- **SC-003**: After this sprint, fewer than 5% of memories written
  through the REST surface are attributed to `system`, where the
  remaining `system` attributions correspond to writes whose
  configured agent name is genuinely `system`.
- **SC-004**: After the one-shot repair runs on the live store, the
  share of historical memories attributed to `system` decreases by
  at least one full author's worth of writes. Every remaining row
  attributed to `system` corresponds to a write whose original
  author was genuinely the seeded system identity; every
  unrecoverable row is attributed to `lost-author` and accounted
  for in the repair report.
- **SC-005**: An operator clicking any Summary cell in the Authors
  view reaches the memory details pane on the first click, on every
  memory in a sample of at least 20 rows across multiple authors.
- **SC-006**: A bench-clean run after a full-corpus seed restores
  the store to its pre-run memory counts (exact match per author,
  per memory kind), using only an author-based purge.
- **SC-007**: The refreshed `perf-baseline.md` reports the documented
  sample target (100 samples across 10 queries) and the harness run
  that produced it completes without triggering the kwi #26 wedge.
- **SC-008**: The Phase 6 MCP test file passes 10 consecutive runs
  under default cargo test parallelism with no prior cleanup.

## Assumptions

- The kwi #26 root cause is on the server side of the connection
  lifecycle (a handler or layer that drops a response/body without
  fully consuming the request); the per-connection timeout and
  per-peer cap together suffice without needing kernel TCP
  keepalive changes.
- The existing event provenance retained in the store is rich enough
  to recover the true author for the majority of `system`-stamped
  historical rows; rows without recoverable provenance are accepted
  as residual `system`.
- The viewport's existing memory details route already supports the
  memory kinds the Authors view exposes; only the link target needs
  fixing, not the destination pane.
- The full-corpus perf rerun continues to use the contract-defined
  corpus size (10k facts / 50k knowledge items) and the documented
  sample target.
- Existing klams deployments where bearer tokens are not yet
  annotated with agent names must remain valid; their writes are
  attributed to a documented default author rather than rejected.

## Dependencies

- The one-shot repair (Story 3) depends on the attribution wiring
  (Story 2) being in place so it has a defined target schema for
  reassignment.
- The full-corpus perf rerun (Story 5) depends on Story 1
  (service stays up under load), Story 2 (seeder can write as
  `klams-bench`), and the corresponding bench-clean parity (FR-011).
- All other stories are independent and can be developed in
  parallel.
