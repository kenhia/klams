# Feature Specification: Operationalize Ingestion

**Feature Branch**: `010-operationalize-ingestion`  
**Created**: 2026-06-09  
**Status**: Draft  
**Input**: User description: "Sprint 010 — operationalize ingestion: run the systemd switchover on kubs0 so klams-scanner and klams-monitor (built CI-green in sprint 003) are installed, enabled, and active; verify end-to-end ingestion of ~/src and ~/obsidian so knowledge memory is demonstrably no longer empty; retire the legacy python looper once the Rust monitor is at parity; fold in service bugs kwi #32 (Authors counts.writes excludes knowledge) and kwi #33 (bench-clean Qdrant ?wait=true); and run a timeboxed TokenMaster integration spike against real indexed data."

> **Note**: The kwi #32 / #33 descriptions above are quoted from the
> original request. Both items are **largely already shipped in sprint
> 009** — see Clarifications below and [research.md](research.md) §R1.
> The residual scope this sprint is viewport-render-only (kwi #32) and
> verify-and-close (kwi #33).

This sprint is **deployment + verification of already-built code** (the
scanner/monitor binaries and their unit files shipped CI-green in sprint
003) plus two in-area service-bug items and one exploratory spike. It is
**not** new feature development for the ingestion path. Three scope
fences are fixed up front:

- The Spec Kit → ATV-StarterKit toolchain migration is **explicitly
  deferred** to the next sprint boundary and is out of scope here.
- The TokenMaster work (Story 6) is a **research spike** — its
  acceptance is documented findings plus a go/no-go recommendation, not
  shipped production code in either repository.
- **kwi #32 and kwi #33 are largely already shipped in sprint 009** and
  this sprint only closes their residuals (see
  [research.md](research.md) §R1 for the FR-by-FR reconciliation). For
  **kwi #32**, the API already returns a separate per-author `knowledge`
  count and the viewport already declares the field — the only residual
  is rendering it (Story 4 is **viewport-render-only**). For **kwi #33**,
  `bench-clean` already issues the synchronous (`?wait=true`) delete —
  the only residual is a live verification before closing the item
  (Story 5 is **verify-and-close**, no code change). Neither story
  re-implements backend behaviour.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Scanner and monitor run under systemd on kubs0 (Priority: P1)

The klams operator runs `klams-service` under systemd on `kubs0`, but
the `klams-scanner` timer and the Rust `klams-monitor` service — both
built and CI-green in sprint 003 — were never switched over. Today
`systemctl` shows only the service unit installed; the scanner and
monitor units are absent, so nothing on a schedule is indexing the
filesystem and a legacy python looper is impersonating the monitor.
After this sprint, the operator must be able to run the existing
[deploy/install-systemd.sh](../../deploy/install-systemd.sh) and have the
scanner timer and the Rust monitor installed, enabled, and active
alongside the service, managed entirely through systemd.

**Why this priority**: This is the root cause of the whole sprint —
ingestion is idle because the units that drive it are not deployed.
Every other primary outcome (knowledge populating, the looper retiring,
the spike having real data to recall) is downstream of these two units
running. It is the one piece that unblocks everything else.

**Independent Test**: On `kubs0`, run the installer (dry-run first, then
for real). Afterwards query `systemctl` for the three klams units and
confirm `klams-service.service` (active), `klams-scanner.timer`
(enabled + waiting/active), and `klams-monitor.service` (enabled +
active). Confirm `systemctl list-timers` shows the scanner timer with a
next-elapse time, and that the units survive a host reboot.

**Acceptance Scenarios**:

1. **Given** `kubs0` has only `klams-service.service` installed under
   systemd, **When** the operator runs the install script's `--dry-run`
   mode, **Then** the script must report the exact install + enable
   actions it would take for `klams-scanner.timer` and
   `klams-monitor.service` without making any changes.
2. **Given** the release binaries are present and the dry-run output is
   acceptable, **When** the operator runs the installer for real,
   **Then** all three units must end up installed, the scanner timer and
   monitor service must be `enabled`, and the monitor service must be
   `active (running)`.
3. **Given** the units are installed and enabled, **When** `kubs0`
   reboots, **Then** the monitor must come back `active` and the scanner
   timer must re-arm with a scheduled next-elapse, without manual
   intervention.
4. **Given** the scanner timer is installed, **When** its interval
   elapses (or the operator triggers `klams-scanner.service` once by
   hand), **Then** the scanner unit must run to completion and exit
   cleanly, leaving the timer armed for the next cycle.

---

### User Story 2 - Ingestion populates searchable knowledge end-to-end (Priority: P1)

With the scanner timer running, the operator needs proof that ingestion
actually works against the real corpus, not just that a unit is enabled.
A scan cycle must walk `~/src` and `~/obsidian`, chunk and embed file
contents, and index them into knowledge memory such that new content
becomes findable via klams search within one scan cycle. Today knowledge
memory is effectively empty/stale; after this sprint it must be
demonstrably populated and the Phase 3 exit criterion from
[plan.md](../planning/plan.md) ("a new note in the Obsidian vault is
searchable in klams within one scan cycle") must hold on the live host.

**Why this priority**: A running timer that indexes nothing is worthless.
This story is the actual goal — klams becoming self-populating and
trustworthy as a data source. It is also the hard dependency for the
TokenMaster spike (Story 6), which cannot be honestly evaluated against
an empty memory store.

**Independent Test**: Before a scan, capture the knowledge-memory item
count and run a search for a known-unindexed term. Drop a sentinel note
containing a unique token into `~/obsidian` (and, separately, a sentinel
file under a scanned `~/src` path). Let one scan cycle run. Confirm the
knowledge count increased, the sentinel token is returned by
`memory_search`, and the result's source attribution points at the
sentinel file.

**Acceptance Scenarios**:

1. **Given** the scanner timer is active and knowledge memory is empty or
   stale, **When** a scan cycle completes against `~/src` and
   `~/obsidian`, **Then** the knowledge-memory item count must increase
   from its pre-scan value and remain queryable across a service restart.
2. **Given** a sentinel note containing a unique token is placed in
   `~/obsidian` before a scan, **When** the next scan cycle completes,
   **Then** a klams search for that token must return the sentinel note
   with source attribution identifying the originating file.
3. **Given** the scanner honours `.gitignore` / `.klamsignore`, **When**
   a scan walks `~/src`, **Then** ignored paths must not appear as
   knowledge items, and non-ignored source files must.
4. **Given** a scan cycle has already indexed a file, **When** the file
   is unchanged on the next cycle, **Then** the scanner must not create
   duplicate knowledge items for it (idempotent re-scan).

---

### User Story 3 - Legacy python looper is retired after monitor parity is confirmed (Priority: P1)

The monitor job on `kubs0` is currently performed by a legacy python
looper at `~/src/tools/ksvc-looper/klams_monitor.py`, which predates the
Rust `klams-monitor`. Once the Rust monitor is installed (Story 1) and
confirmed to emit equivalent typed service-lifecycle events (service
up / down / version-changed) into klams, the operator must stop and
decommission the python looper so the two are not both writing events.
The looper must **not** be decommissioned until parity is demonstrated —
no observability gap is acceptable during the switchover.

**Why this priority**: Running both monitors double-reports service
events and muddies attribution; running neither loses service
observability. The cutover must be deliberate and parity-gated, which
makes it a P1 sequencing concern even though the code already exists.

**Independent Test**: With both monitors briefly running, trigger a
service-lifecycle transition (e.g. restart a watched unit). Confirm the
Rust monitor records a typed `Service` event (up/down/version-changed)
with the correct service name and transition. Then stop the python
looper and trigger another transition; confirm the event is still
recorded by the Rust monitor alone and no events are lost.

**Acceptance Scenarios**:

1. **Given** the Rust monitor is active, **When** a watched unit
   transitions between active and inactive, **Then** the monitor must
   record a typed `Service` event capturing the service name and the
   transition kind (up, down, or version-changed).
2. **Given** the Rust monitor has demonstrated event parity with the
   python looper over a representative set of transitions, **When** the
   operator stops and decommissions
   `~/src/tools/ksvc-looper/klams_monitor.py`, **Then** subsequent
   service transitions must continue to be recorded — by the Rust
   monitor alone — with no gap and no duplicate events.
3. **Given** parity has **not** yet been confirmed, **When** the operator
   evaluates the cutover, **Then** the python looper must remain running
   so that service observability is never interrupted.

---

### User Story 4 - Author counts include knowledge writes (Priority: P2)

> **Residual-only (kwi #32).** The backend already separates the two
> write kinds: the API returns a per-author `knowledge` count and the
> viewport `AuthorCounts` type already declares the field (shipped
> sprint 009). The **only** gap is that the viewport's Authors pages
> render `writes` and never display `knowledge`. This story is therefore
> **viewport-render-only** — no backend, store, API, or type change. See
> [research.md](research.md) §R1.

In the viewport Authors view, the **Writes** column and the per-author
`writes · events · soft-deletes · restores` summary display facts only —
the per-author `knowledge` count the API already returns is never
rendered. The result is that an author who has indexed tens of thousands
of knowledge rows (e.g. `klams-bench`, or now the scanner's own author
once Story 2 lands) appears to have done nothing. With ingestion newly
live and producing large volumes of knowledge, hiding that count becomes
materially misleading. After this sprint, the per-author surface must
**render** the knowledge count alongside writes. Tracked as kwi #32.

**Why this priority**: A correctness/honesty gap on a per-author surface
that ingestion makes acutely visible, but it causes no data loss or
service impact and the fix is a small render change. It rides naturally
alongside the ingestion work that exposes it.

**Independent Test**: For a known author that has indexed knowledge items
(verifiable directly in the underlying knowledge store by that author's
identity), open the per-author detail surface and confirm the displayed
counts now render that author's knowledge writes rather than omitting
them.

**Acceptance Scenarios**:

1. **Given** an author has indexed knowledge items, **When** the operator
   views that author's per-author detail surface, **Then** the displayed
   counts must include a knowledge measure that matches the author's
   actual knowledge-item count in the store.
2. **Given** an author has both facts and knowledge items, **When** the
   counts are displayed, **Then** facts and knowledge must be
   distinguishable rather than conflated, so the operator can tell the
   two memory kinds apart.

---

### User Story 5 - bench-clean drains Qdrant synchronously (Priority: P2)

> **Verify-and-close (kwi #33).** The `bench-clean` recipe already issues
> the synchronous knowledge-store delete (`points/delete?wait=true`,
> shipped sprint 009). This story carries **no code change** — its work
> is a single live verification on `kubs0` that a clean leaves zero
> residue, after which kwi #33 is closed. See [research.md](research.md)
> §R1.

The `bench-clean` workflow previously issued an asynchronous
knowledge-store delete that returned success before the points were
removed; a documented sprint-009 walkthrough found hundreds of points
still present for the bench author after a clean. Sprint 009 fixed this
by making the delete block until committed (`?wait=true`). With ingestion
now continuously writing, this sprint confirms the fix holds under live
load before retiring the item. Tracked as kwi #33.

**Why this priority**: A tooling-hygiene item that affects the
reliability of bench/clean cycles; the fix already shipped, so the
remaining effort is a contained, low-risk verification. Bundled because
it exercises the same service/ingestion path as the switchover.

**Independent Test**: Seed the bench corpus, run a clean, and confirm the
knowledge store holds zero points for the bench author immediately after
the clean returns — without a follow-up manual drain.

**Acceptance Scenarios**:

1. **Given** a bench seed has populated knowledge memory, **When** the
   operator runs bench-clean, **Then** the clean call must not return
   success until the knowledge-store delete has been committed.
2. **Given** bench-clean has returned, **When** the operator inspects the
   knowledge store for the bench author, **Then** zero residual points
   must remain (no follow-up manual delete required).

---

### User Story 6 - TokenMaster integration spike against real data (Priority: P3)

With ingestion live and knowledge memory populated, the operator runs a
timeboxed, exploratory spike to validate the TokenMaster integration
[analysis](../planning/tokenmaster-integration/analysis.md). TokenMaster
(TMX) is run first against a **Python** repository — where its graph
supplier is proven — rather than the klams Rust repo, where the call
graph is documented to be sparse. TMX's routing agent is then pointed at
the klams MCP endpoint so it uses klams `memory_search` / `memory_add` as
its durable, semantic cross-session memory layer (Option A in the
analysis — the actual integration seam). The spike ships **no production
code**; its deliverable is findings captured back into
[specs/planning/tokenmaster-integration/](../planning/tokenmaster-integration/README.md)
and a go/no-go recommendation on whether to pull the "Lightweight graph
memory" backlog item forward.

**Why this priority**: Highest-learning, lowest-commitment work, and it
is strictly downstream of Stories 1–2 — it is meaningless against an
empty memory store. It is exploratory by design: a research story whose
output is a decision, not a feature.

**Independent Test**: Confirm that (a) TMX built a usable graph against a
Python repo, (b) the routing agent, when asked a recall-shaped question,
reached klams `memory_search` and surfaced real indexed content produced
by Stories 1–2, (c) a durable finding written via `memory_add` is later
recallable, and (d) a written findings document with an explicit go/no-go
recommendation exists under the tokenmaster-integration planning folder.

**Acceptance Scenarios**:

1. **Given** ingestion has populated knowledge memory, **When** the spike
   wires TMX's routing agent to the klams MCP endpoint and asks a
   recall-shaped question, **Then** the agent must use klams
   `memory_search` and return real indexed content (not an empty result).
2. **Given** the spike records a durable finding via klams `memory_add`,
   **When** the same finding is queried in a later turn or session,
   **Then** it must be recallable from klams.
3. **Given** the spike is timeboxed, **When** the time box is reached,
   **Then** a findings document with an explicit go/no-go recommendation
   on the "Lightweight graph memory" backlog item must be committed to
   the tokenmaster-integration planning folder — regardless of whether
   the integration looked promising.

---

### Edge Cases

- The install script is re-run after the units are already installed —
  it must be idempotent (re-install/enable without error and without
  disrupting the running service).
- A required dependency for the units is missing on the host (e.g. the
  database unit the service depends on) — the installer must fail with a
  clear, actionable error rather than leaving a half-installed state.
- A scan encounters a file it cannot read (permissions) or a binary/
  non-text file — the scan cycle must skip it and continue rather than
  aborting the whole cycle.
- The scanner and the python looper are both active during the Story 3
  parity window — duplicate service events recorded in that window must
  be recognizable as such and must not be misread as the steady-state
  behaviour after the looper is retired.
- A scan cycle overruns its timer interval (large initial index of
  `~/src` + `~/obsidian`) — a new cycle must not stack on top of an
  in-progress one in a way that double-indexes or corrupts state.
- The TokenMaster spike finds the integration unviable — that is a valid
  outcome; the negative finding and its reasoning must still be recorded.

## Requirements *(mandatory)*

### Functional Requirements

**Systemd switchover (Story 1)**

- **FR-001**: The existing install script MUST install, enable, and
  start `klams-scanner.timer` and `klams-monitor.service` on `kubs0`
  alongside the already-running `klams-service.service`, using the
  binaries and unit files produced in sprint 003.
- **FR-002**: The install script MUST support a no-op dry-run that
  reports the exact install/enable actions without modifying the host.
- **FR-003**: The install MUST be idempotent — re-running it on a host
  where the units are already present MUST NOT error or disrupt the
  running service.
- **FR-004**: After install, the scanner timer MUST be enabled with a
  scheduled next-elapse and the monitor service MUST be active; both MUST
  survive a host reboot and re-arm/restart automatically.
- **FR-005**: The day-to-day operation of the scanner and monitor MUST be
  managed through systemd (timer + service units), not through an
  ad-hoc looper or manual invocation.

**End-to-end ingestion (Story 2)**

- **FR-006**: A scan cycle MUST walk `~/src` and `~/obsidian`, chunk and
  embed file contents, and index them into knowledge memory. (`~` here is
  shorthand for Ken's trees; because the scanner runs as `User=klams`,
  these MUST be deployed as **absolute** roots — `/home/ken/src`,
  `/home/ken/obsidian` — not `~`. See [research.md](research.md) §R2 and
  [contracts/scanner-config.md](contracts/scanner-config.md) C1.)
- **FR-007**: Content indexed by a scan cycle MUST become findable via
  klams search within one scan cycle of its appearance on disk (the
  Phase 3 exit criterion).
- **FR-008**: The scanner MUST honour `.gitignore` / `.klamsignore` so
  ignored paths are not indexed.
- **FR-009**: A re-scan of unchanged content MUST NOT create duplicate
  knowledge items (idempotent ingestion).
- **FR-010**: Indexed knowledge items MUST carry source attribution
  identifying the originating file so search results are traceable back
  to disk.
- **FR-011**: Indexed knowledge MUST persist across a service restart.

**Monitor cutover (Story 3)**

- **FR-012**: The Rust monitor MUST record typed `Service` events
  capturing the watched service name and the transition kind (service
  up, service down, version changed) when a watched unit transitions.
- **FR-013**: The legacy python looper MUST NOT be decommissioned until
  the Rust monitor has demonstrated event parity over a representative
  set of service transitions.
- **FR-014**: After the python looper is retired, service-lifecycle
  events MUST continue to be recorded by the Rust monitor alone, with no
  observability gap and no duplicate-source events.

**Author knowledge counts (Story 4 / kwi #32 — viewport-render-only)**

> The API already returns a per-author `knowledge` count and the viewport
> already declares the field; these FRs are satisfied by **rendering**
> the existing value, not by new backend work.

- **FR-015**: The per-author surface MUST render the knowledge count the
  API already returns, so an author that has only indexed knowledge no
  longer displays as having done nothing.
- **FR-016**: Facts and knowledge writes MUST be displayed as distinct
  measures on the per-author surface so the two memory kinds are not
  conflated into a single number.

**bench-clean drainage (Story 5 / kwi #33 — verify-and-close)**

> The synchronous (`?wait=true`) delete already shipped; these FRs are
> satisfied by **verifying** the committed behaviour on the live host,
> not by changing the recipe.

- **FR-017**: The bench-clean knowledge-store delete MUST block until the
  delete is committed (already implemented); the sprint MUST confirm this
  on the live host.
- **FR-018**: After bench-clean returns, zero residual knowledge points
  MUST remain for the bench author without any follow-up manual drain —
  verified live before closing kwi #33.

**TokenMaster spike (Story 6)**

- **FR-019**: The spike MUST run TokenMaster against a Python repository
  (where its graph supplier is proven) rather than the klams Rust repo.
- **FR-020**: The spike MUST wire TokenMaster's routing agent to the
  klams MCP endpoint so recall-shaped questions are served by klams
  `memory_search` and durable findings are written via klams
  `memory_add`, against the real data produced by Stories 1–2.
- **FR-021**: The spike MUST capture its findings — including a go/no-go
  recommendation on the "Lightweight graph memory" backlog item — into
  the `specs/planning/tokenmaster-integration/` folder, and MUST NOT ship
  production code in the klams or TokenMaster repositories.

### Key Entities

- **Systemd unit set**: The three klams units (`klams-service.service`,
  `klams-scanner.timer` driving `klams-scanner.service`, and
  `klams-monitor.service`) plus the install script that deploys and
  enables them on `kubs0`.
- **Scan corpus**: The `~/src` and `~/obsidian` trees the scanner walks,
  filtered by `.gitignore` / `.klamsignore`, chunked and embedded into
  knowledge memory.
- **Knowledge item**: An embedded chunk of scanned content with source
  attribution, persisted in the knowledge store and findable via search.
- **Service-lifecycle event**: A typed `Service` event (up / down /
  version-changed) recorded by the monitor when a watched unit
  transitions — the parity yardstick for retiring the python looper.
- **Per-author counts**: The aggregate of an author's writes (facts) and
  knowledge, plus events, soft-deletes, and restores shown on the
  per-author surface. The API already returns facts and knowledge as
  distinct counts; kwi #32 is the viewport rendering the knowledge one.
- **TokenMaster integration seam**: TMX's routing agent pointed at the
  klams MCP endpoint, using `memory_search` / `memory_add` as its
  durable semantic memory layer.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: After the switchover on `kubs0`, all three klams units are
  present under systemd; the scanner timer and monitor service are
  enabled, the monitor is active, and the scanner timer shows a scheduled
  next-elapse — verifiable from `systemctl` status/timer listings.
- **SC-002**: The scanner and monitor units survive a host reboot: after
  a reboot the monitor returns to active and the scanner timer re-arms,
  with no manual intervention.
- **SC-003**: Knowledge memory is demonstrably no longer empty — the
  knowledge-item count after the first full scan of `~/src` and
  `~/obsidian` is materially greater than its pre-sprint value (which was
  effectively idle/stale).
- **SC-004**: A sentinel note placed in `~/obsidian` before a scan is
  returned by klams search within one scan cycle, with source
  attribution pointing at the sentinel file (the Phase 3 exit criterion,
  demonstrated on the live host).
- **SC-005**: A re-scan of an unchanged corpus produces no net increase
  in knowledge-item count attributable to duplication (idempotent
  ingestion confirmed across at least two consecutive cycles).
- **SC-006**: The Rust monitor records a typed `Service` event for every
  watched-service transition in a representative parity test, matching
  what the python looper recorded for the same transitions — and the
  python looper is retired only after that parity is shown.
- **SC-007**: After the looper is retired, a subsequent service
  transition is recorded by the Rust monitor alone, with no gap and no
  duplicate-source event.
- **SC-008**: For an author that has only indexed knowledge, the
  per-author surface shows a non-zero count that matches the author's
  actual knowledge-item count in the store (kwi #32 closed).
- **SC-009**: A bench-clean run leaves zero residual knowledge points for
  the bench author with no follow-up manual drain (kwi #33 closed).
- **SC-010**: The TokenMaster spike produces a committed findings
  document under `specs/planning/tokenmaster-integration/` with an
  explicit go/no-go recommendation on the "Lightweight graph memory"
  backlog item, demonstrating that the routing agent recalled real
  indexed content via klams `memory_search` — and ships no production
  code.

## Assumptions

- The scanner and monitor binaries and their unit files, built CI-green
  in sprint 003, are unchanged and correct; this sprint deploys and
  verifies them rather than modifying ingestion behaviour. Any fix needed
  is treated as a defect surfaced by deployment, not planned feature work.
- `kubs0` already satisfies the units' host dependencies (the database
  unit the service depends on, release binaries on disk, the `klams`
  system user and state/config directories) — the same preconditions the
  service install already relies on.
- The scanner author identity used for ingestion writes is a registered
  author, so its knowledge writes are attributable on the per-author
  surface (the surface kwi #32 corrects).
- The python looper and the Rust monitor emit comparable
  service-lifecycle semantics (up / down / version-changed), so a
  transition-by-transition comparison is a fair parity test.
- The TokenMaster spike runs against systems as they ship today (no new
  TMX or klams features), per the integration analysis; the routing-agent
  wiring is a template/config edit, not code.
- The Spec Kit → ATV-StarterKit migration is deferred to the next sprint
  boundary and contributes no work to this sprint.

## Dependencies

- Story 2 (verified ingestion) depends on Story 1 (the scanner timer
  being installed and running).
- Story 3 (retiring the python looper) depends on Story 1 (the Rust
  monitor being installed) and on demonstrated event parity before the
  cutover.
- Story 6 (the TokenMaster spike) depends on Stories 1 and 2 having
  populated knowledge memory with real data to recall.
- Stories 4 (kwi #32) and 5 (kwi #33) are independent of the switchover
  and of each other, and are residual-only: Story 4 is a viewport render
  change and Story 5 is a live verification of already-shipped behaviour
  — though Story 4's effect is most clearly demonstrated once ingestion
  (Story 2) has produced knowledge volume.
