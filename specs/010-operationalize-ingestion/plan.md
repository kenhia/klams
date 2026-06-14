# Implementation Plan: Operationalize Ingestion

**Branch**: `010-operationalize-ingestion` | **Date**: 2026-06-09 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `specs/010-operationalize-ingestion/spec.md`

## Summary

This sprint turns klams from "service-only" into "self-populating" by
deploying the `klams-scanner` timer and the Rust `klams-monitor` service
on `kubs0` — both built and CI-green in sprint 003 but never switched
over — then proving ingestion works end-to-end and retiring the legacy
python looper once the Rust monitor is at parity. Two tracked service
bugs (kwi #32, #33) ride along, and a timeboxed TokenMaster spike runs
last against the now-real data.

**Material scope correction surfaced during planning** (see
[research.md](research.md) §R1): the two bug-fix stories are **already
shipped**, not open backend work.

- **kwi #33 (US5)** — `just bench-clean` already issues
  `points/delete?wait=true` (justfile line ~352, shipped sprint 009
  T038). Remaining work is **verify on the live host + close the kwi
  item** — zero code.
- **kwi #32 (US4)** — the backend already returns a separate
  `counts.knowledge` (API `AuthorCounts.knowledge` ←
  `writes_knowledge`, shipped sprint 009 T048/T049). The viewport
  `AuthorCounts` TS interface already carries `knowledge`. The **only**
  remaining gap is that the Authors list/detail Svelte renders
  `counts.writes` and never displays `counts.knowledge`. This is a
  **viewport-render-only** change, matching the backlog note "the gap is
  purely on the SvelteKit render side."

Technical approach: the primary work is **deployment + verification**,
not Rust changes. Drive the existing
[deploy/install-systemd.sh](../../deploy/install-systemd.sh) (whose
`ENABLE_LIST` already covers all three units) on `kubs0`; author real
`/etc/klams/scanner.toml` and `/etc/klams/monitor.toml` configs; verify
ingestion with a sentinel-note probe; run a parity window before
retiring the looper. The viewport gains a `knowledge` count cell; the
spike is documentation-only.

## Technical Context

**Language/Version**: Rust 1.83 (workspace edition 2021) — only the
viewport (US4) and possibly a small scanner config/path defect (if
deployment surfaces one) touch code; TypeScript (SvelteKit) for the
viewport render fix.
**Primary Dependencies**: existing `klams-scanner` (`ignore`, `rusqlite`
cursor, `sha2`), `klams-monitor` (`tokio::process` over `systemctl`),
`klams-client` (HTTP); systemd (timer + oneshot + service units);
SvelteKit + vitest for the viewport.
**Storage**: Postgres 16 (facts, events, authors), Qdrant
(`knowledge_items`); scanner SQLite cursor under `StateDirectory=klams`
(`/var/lib/klams`).
**Testing**: `cargo test` workspace + `just gate`; vitest +
`svelte-check` for the viewport cell; live-host verification on `kubs0`
for the deployment stories (systemd status, sentinel-note search).
**Target Platform**: Linux server (`kubs0`) for service + scanner +
monitor + datastores; Windows/Linux desktop for the viewport.
**Project Type**: Rust workspace with a SvelteKit/Tauri sibling
(`viewport/`), established in sprints 001–009.
**Performance Goals**: No new latency budgets. The initial full scan of
`~/src` + `~/obsidian` may overrun the 1 h timer interval once — the
oneshot unit + `OnUnitActiveSec=1h` prevents cycle stacking (the next
cycle only arms after the prior one finishes).
**Constraints**: No new external services. Deployment must respect the
scanner unit's hardening (`User=klams`, `ProtectHome=read-only`) — see
the home-path access risk in [research.md](research.md) §R2. The looper
cutover is parity-gated (no observability gap). The spike ships no
production code in either repo.
**Scale/Scope**: ~2 viewport files touched (US4); 2 example config files
authored; 0–1 small scanner defect fixes if deployment surfaces one; the
rest is operational verification + one findings document.

## Constitution Check

*GATE: must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Compliance |
|-----------|-----------|
| I. Spec-Driven Development | `spec.md` is in place with FRs/SCs; this plan and downstream artifacts live in the same feature directory. The scope correction (kwi #32/#33 already shipped) is recorded in `research.md` §R1 and reconciled against the spec's FR-015…FR-018 there rather than silently. ✅ |
| II. Test-Driven Development | The only new production code (US4 viewport cell) gets a vitest assertion first. Deployment stories (US1–US3) are verified by executable acceptance probes (systemd status, sentinel-note search, parity transition) documented in `quickstart.md` — the SDD analogue of tests for ops work. kwi #33 is verification-only against existing behaviour. ✅ |
| III. Code Standards Gate | Viewport change keeps `just gate` + `svelte-check` green. No new Rust deps. If deployment surfaces a scanner path/config defect, its fix follows TDD and the gate. ✅ |
| IV. Documentation | Phase 1 produces `quickstart.md` (operator switchover + verification runbook). Polish updates `docs/setup.md` (scanner/monitor config + install), `docs/architecture.md` (ingestion now live; looper retired), `docs/usage.md` (Authors knowledge count). ✅ |
| V. Quality & Observability | Scanner/monitor already emit structured `tracing` + Prometheus metrics; verification asserts they are scraped. The cutover is observable (parity window). No raw stack traces. ✅ |
| VI. Simplicity & Intentional Design | This is the principle the scope correction serves: **do not re-implement kwi #32/#33 backend work that already shipped.** US4 shrinks to a render cell; US5 shrinks to verify-and-close. No new services, no speculative abstractions. The deployment reuses the existing idempotent installer rather than new tooling. ✅ |

Gate: **pass**. No violations to justify. The spec's FR-015…FR-018 are
satisfied largely by already-shipped code; the plan tracks the residual
(viewport render + live verification) rather than duplicating shipped
work — see [research.md](research.md) §R1 for the FR-by-FR reconciliation.

**Post-design re-check (after Phase 1)**: Still **pass**. The Phase 1
artifacts added no new services, dependencies, endpoints, or schema —
`data-model.md` confirms zero DB change and a render-only US4;
`contracts/` document deployment/verification interfaces rather than new
code surfaces. Principle VI is reinforced, not eroded, by the design.

## Project Structure

### Documentation (this feature)

```text
specs/010-operationalize-ingestion/
├── plan.md              # this file
├── research.md          # Phase 0 — scope reconciliation + deployment-risk decisions
├── data-model.md        # Phase 1 — config/entity shapes (no DB schema change)
├── quickstart.md        # Phase 1 — operator switchover + verification runbook
├── contracts/
│   ├── README.md
│   ├── scanner-config.md       # /etc/klams/scanner.toml shape + root/path rules
│   ├── monitor-parity.md       # parity-window procedure gating looper retirement
│   └── author-counts-ui.md     # viewport AuthorCounts.knowledge render contract (kwi #32)
├── checklists/
│   └── requirements.md  # spec quality (already created)
└── tasks.md             # produced by /speckit.tasks
```

### Source Code (repository root)

```text
crates/
├── klams-scanner/              # NO planned change — deployed + verified as-is.
│   ├── src/main.rs             #   Only touched if deployment surfaces a path/
│   └── src/{walk,chunk,cursor,publish}.rs  #   config defect (e.g. ~ vs absolute roots).
└── klams-monitor/              # NO planned change — deployed + verified as-is.

viewport/                       # US4 (kwi #32) — render-only
├── src/routes/authors/+page.svelte         # add a Knowledge cell beside Writes
├── src/routes/authors/[id]/+page.svelte    # pass counts.knowledge to the detail summary
├── src/lib/types.ts            # AuthorCounts.knowledge already present — no change
└── src/routes/authors/*.test.ts            # NEW vitest: knowledge count rendered

deploy/
├── install-systemd.sh          # reused as-is (ENABLE_LIST already correct)
├── klams-scanner.{service,timer}   # reused as-is (verify ProtectHome/roots)
├── klams-monitor.service       # reused as-is
└── config/
    ├── scanner.example.toml    # NEW: example scanner config (roots, url, token, interval)
    └── monitor.example.toml    # NEW: example monitor config (watched units, url, token)

justfile                        # kwi #33 — already has ?wait=true; verify only, no edit

docs/
├── setup.md                    # scanner/monitor install + config section
├── architecture.md             # ingestion live; python looper retired
└── usage.md                    # Authors knowledge count surfaced
```

**Structure Decision**: Existing Rust workspace + `viewport/` sibling,
unchanged. The sprint's center of gravity is `deploy/` + live-host
operation, not `crates/`. The only planned production code is the
viewport render cell (US4). Scanner/monitor crates are touched **only
if** deployment surfaces a defect (treated as a deployment-surfaced bug
per the spec's Assumptions, not planned feature work).

## Complexity Tracking

> No constitution violations. Table intentionally empty.

The notable planning decision is *removing* work, not adding it: US4 and
US5 are reduced from "implement backend bug fixes" to "render a count
the backend already returns" and "verify an already-shipped fix and
close the item," respectively. Rationale and the FR-by-FR reconciliation
are in [research.md](research.md) §R1.
