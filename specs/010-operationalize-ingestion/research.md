# Phase 0 Research: Operationalize Ingestion

**Feature**: `010-operationalize-ingestion` | **Date**: 2026-06-09
**Plan**: [plan.md](plan.md)

This document resolves the unknowns and risks for the sprint. The
headline output is **R1** — a correction to the spec's assumption that
kwi #32 and #33 are open backend bugs. They are not; they shipped in
sprint 009. The rest captures the deployment risks that actually matter
for the systemd switchover.

---

## R1 — kwi #32 and #33 are already shipped (scope correction)

**Decision**: Reframe US4 to viewport-render-only and US5 to
verify-and-close. Do **not** re-implement the backend changes the spec's
FR-015…FR-018 imply.

**What the code shows:**

- **kwi #33 / US5 / FR-017–FR-018** — `just bench-clean` already POSTs to
  `…/points/delete?wait=true` (justfile, the bench-clean recipe). Sprint
  009 `tasks.md` records this under T038: *"bench-clean should append
  `?wait=true`… Recipe now uses `points/delete?wait=true` so the call
  blocks until the operation commits."* **Status: shipped.** Residual
  work: run one live bench seed → clean → confirm zero residue on
  `kubs0`, then close kwi #33.

- **kwi #32 / US4 / FR-015–FR-016** — the backend already separates the
  two write kinds:
  - `crates/klams-api/src/handlers/authors.rs` — `AuthorCounts` has both
    `writes: i64` **and** `knowledge: i64` (← `row.writes_knowledge`).
  - `crates/klams-store/src/composite.rs` — `list_authors_v1` and
    `get_author_v1` populate `writes_knowledge` via
    `QdrantStore::count_live_knowledge_by_author` (sprint 009 T048/T049).
  - `viewport/src/lib/types.ts` — the `AuthorCounts` interface already
    declares `knowledge: number`.

  The **only** remaining gap: the viewport renders `counts.writes` and
  never displays `counts.knowledge`:
  - `viewport/src/routes/authors/+page.svelte` — a single `Writes`
    column showing `{a.counts.writes}`.
  - `viewport/src/routes/authors/[id]/+page.svelte` — passes
    `writes={author.counts.writes}` only.

  **Status: backend shipped; viewport render outstanding.** This matches
  the backlog note for this item: *"the gap is purely on the SvelteKit
  render side."*

**FR-by-FR reconciliation** (so the spec stays the source of truth):

| FR | Spec intent | Reality | Residual this sprint |
|----|-------------|---------|----------------------|
| FR-015 | Per-author counts account for knowledge writes | API returns `counts.knowledge`; viewport hides it | Render `counts.knowledge` in the Authors list + detail |
| FR-016 | Facts and knowledge distinguishable, not conflated | Already two distinct fields end-to-end | Show both as separate cells/labels |
| FR-017 | bench-clean delete blocks until committed | `?wait=true` already in the recipe | Live verification only |
| FR-018 | Zero residual points after bench-clean | Achieved by `?wait=true` | Live verification + close kwi #33 |

**Rationale**: Constitution Principle VI (Simplicity / YAGNI) forbids
re-doing shipped work. Surfacing this in Phase 0 — rather than letting
`/speckit.tasks` emit redundant backend tasks — is the whole point of the
research gate.

**Alternatives considered**:
- *Implement FR-015…FR-018 as written* — rejected: duplicates shipped
  code, risks regressing the sprint-009 implementation.
- *Drop US4/US5 from the sprint entirely* — rejected: the viewport still
  visibly under-reports (kwi #32 is genuinely user-facing-open), and kwi
  #33 needs a live confirmation before closing. Keep both, scoped to the
  true residual.

---

## R2 — Scanner runs as `User=klams` with `ProtectHome=read-only`: home-path access

**Decision**: The deployed `/etc/klams/scanner.toml` MUST use **absolute
roots** (`/home/ken/src`, `/home/ken/obsidian`), not `~`, and the
deployment MUST verify the `klams` system user can actually read
`/home/ken/...` under the unit's `ProtectHome=read-only` sandbox.

**Why this is the real risk of the sprint:**

- `deploy/klams-scanner.service` runs `ExecStart=/usr/local/bin/klams-scanner --once`
  as `User=klams`, `Group=klams`, with `ProtectHome=read-only`.
- The scanner's default roots (`crates/klams-scanner/src/main.rs`
  `default_roots()`) are `~/src` and `~/obsidian`. `~` expands relative
  to the **running user's** home — i.e. the `klams` system user
  (`/var/lib/klams` or `/home/klams`), **not** `/home/ken`. Left on
  defaults, the scanner would walk the wrong (empty) tree.
- `ProtectHome=read-only` keeps `/home` **readable** (read-only is not
  `tmpfs`/inaccessible), so reading `/home/ken` is permitted by the
  sandbox — but **Unix permissions** still apply: if `/home/ken` is mode
  `0700`, the `klams` user cannot traverse it.

**Mitigations to validate during deployment (US1/US2):**
1. Author `/etc/klams/scanner.toml` with explicit absolute `roots =
   ["/home/ken/src", "/home/ken/obsidian"]`.
2. Confirm `klams` can read the trees: either group-grant (`klams` in a
   group with `r-x` on the paths) or, if that is unacceptable, decide
   whether the scanner should instead run as `ken` (a deviation from the
   shipped unit — record it if chosen).
3. If neither is clean, treat it as a deployment-surfaced defect (per the
   spec Assumptions) and fix minimally — e.g. a unit `ReadOnlyPaths=` /
   `SupplementaryGroups=` addition — rather than broadening the sandbox.

**Alternatives considered**:
- *Relax `ProtectHome` to `read-only`→off* — rejected: weakens the
  shipped hardening for no necessary gain; read-only already allows
  reads.
- *Symlink/bind-mount Ken's trees under `/var/lib/klams`* — rejected as
  over-engineering for a single-host homelab; explicit absolute roots are
  simpler.

---

## R3 — Idempotent re-scan mechanism (FR-009 / SC-005)

**Decision**: Rely on the scanner's existing SQLite mtime cursor; verify
idempotency empirically with two consecutive cycles rather than adding
any dedupe code.

**Basis**: `crates/klams-scanner/src/lib.rs` `scan_root` opens a
`Cursor`, and for each walked file compares `prev.mtime_ns == f.mtime_ns`,
incrementing a `mtime_unchanged` skip metric on a match. Content also
dedupes by `sha256_hex` on the publish path. The cursor lives under
`StateDirectory=klams` (`/var/lib/klams`), which systemd provisions and
persists across runs. So idempotency is a property of already-shipped
code; the sprint's job is to **prove** it (SC-005: two cycles, no net
duplicate growth), not build it.

**Open verification point**: confirm the cursor file path the unit uses
(`state_dir` default `~/.local/state/klams` in config vs systemd
`StateDirectory`=/var/lib/klams) resolves consistently for `User=klams`
— another reason R2's config must be explicit.

---

## R4 — Monitor parity window before retiring the python looper (FR-013 / SC-006)

**Decision**: Run both monitors concurrently for a bounded parity window,
drive a representative set of `systemctl` transitions, and diff the
emitted `Service` events before stopping
`~/src/tools/ksvc-looper/klams_monitor.py`.

**Basis**: `klams-monitor` shells `systemctl is-active <unit>`
(`poll.rs`), diffs against a `PreviousState` cache (`state.rs`), and
posts typed `Service` events via `klams-client` (`publish.rs`) — the
state-diff semantics are unit-tested (sprint 003 T022/T026/T027). Parity
is therefore a runtime comparison, not new code. The procedure is
specified as a contract in
[contracts/monitor-parity.md](contracts/monitor-parity.md).

**Risk captured as an edge case**: during the window both monitors write
events for the same transition — duplicates in that window are expected
and must not be mistaken for steady-state behaviour after cutover
(SC-007).

---

## R5 — TokenMaster spike preconditions (US6)

**Decision**: Gate the spike on US1+US2 completion (real indexed data),
run TMX against a **Python** repo (graphify is proven there; sparse on
Rust/JS-TS per the TMX README), and exercise the integration seam =
TMX routing agent → klams MCP `memory_search`/`memory_add`. Output is a
findings doc + go/no-go on the "Lightweight graph memory" backlog item.

**Basis**: the integration analysis already done in
[../planning/tokenmaster-integration/analysis.md](../planning/tokenmaster-integration/analysis.md)
(Option A is exactly this seam). The spike validates that analysis
against live data; it ships no code (FR-021). No further research needed
beyond confirming the klams MCP endpoint is reachable with a scoped
bearer for the agent.

---

## Summary of decisions

| # | Decision | Effect on sprint |
|---|----------|------------------|
| R1 | kwi #32/#33 already shipped | US4 → viewport render only; US5 → verify + close |
| R2 | Scanner needs absolute roots + verified `klams` read access | The primary deployment risk; gates US2 |
| R3 | Reuse mtime cursor; prove idempotency | SC-005 is verification, not code |
| R4 | Parity window before looper retirement | US3 procedure is a runtime diff |
| R5 | Spike gated on US1+US2, Python repo, MCP seam | US6 stays documentation-only |

No `NEEDS CLARIFICATION` markers remain.
