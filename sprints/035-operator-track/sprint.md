# Sprint 035 — Operator track: someone else's klams

**Proposal:** korg:781 · **Covers:** #776, #777, #778, #779 · **Branch:** `035-operator-track`

## Goal

Make klams installable and runnable by an **Operator** — someone who
downloads the repo, follows one guide, and runs klams as *their*
personal memory system in *their* environment. Per the planning doc
(`sprints/planning/generalize-klams.md`), the code is already portable —
no hostname or Ken-shaped concept in any code path — so this sprint is
**defaults, docs, and posture**, not an architecture refactor.

## Scope

1. **#776 (M) — first-hour papercuts.** Viewport `kubs0:7777` default,
   justfile Ken-machine defaults fail loudly (the #682 pattern),
   `scanner.example.toml` placeholder-that-errors, provision script
   renders `scanner.toml`/`monitor.toml` with tokens filled, reranker in
   the closing compose line, generic monitor example, `pg_bin_dir`
   documented. Acceptance: a stranger's first hour contains zero
   Ken-shaped strings — every default works generically or fails loudly
   with instructions.
2. **#777 (M) — `docs/install.md` written forward**, README front door
   off the sprint quickstarts, a portable agent-routing policy, the
   plain-HTTP/TLS truth, the week-one empty-corpus expectation.
3. **#778 (S) — posture set**: support posture + hardware stance in the
   README, the versioning/upgrade-contract paragraph, the AGENTS.md
   portability carve-out, Contributor explicitly deferred.
4. **#779 (S) — first-run smoke**: one command that proves an empty
   install end-to-end; install.md ends with it. Designated first-install
   target: **ksandbox** (no GPU → forces the CPU branch of the decision
   tree; see #779's comment).

## Acceptance

- `just gate` green; integration suite green.
- A fresh-eyes read of install.md gets from `git clone` to a successful
  `memory_search` using only that document.
- `just smoke` on an empty store: green means "your install works".

## Decisions

(chronicled as made — see "Outcome" below at ship time)

- **Versioning/upgrade contract (#778):** stated honestly as "run
  `main`, expect to re-scan occasionally." No release tags yet — the
  version's PATCH segment is a sprint number, not a compatibility
  signal; cutting tags starts when a real second operator exists to
  consume them. Recorded in README.
- **Contributor stays deferred (#778):** no CONTRIBUTING.md, issue
  templates, or PR policy until a real contributor materializes
  (Ken, 2026-07-29). Deliberate, recorded in README so it isn't
  re-litigated.
- **AGENTS.md carve-out (#778):** the line is redrawn as
  *operator-facing surfaces generic-or-fail-loudly; architecture stays
  purpose-built*. Ken-specific values live in config/env, never in
  defaults.
- **Scanner roots now fail loudly (#776):** a configured root that does
  not exist aborts startup with an actionable error instead of
  warn-and-continue. Rationale: `scanner.example.toml` shipping
  `/home/ken/src` "worked" by silently scanning nothing on any other
  machine — the bug class this sprint exists to kill.
- **No `--cpu` provision flag:** the CPU branch is documented edits in
  install.md, not tooling. #780 (compose-mode + CPU override) stays
  gated on the first real operator's friction list.
- **CPU tag is `cpu-1.9`, not `cpu-1.7` (#779 finding):** the live
  ksandbox first-install crashlooped TEI on `cpu-1.7` — the deploy
  compose command passes `--auto-truncate false`, and pre-1.8 clap
  treats `--auto-truncate` as a value-less switch (`unexpected
  argument 'false'`). "cpu-1.7 is CI-proven" was true only of CI's
  *minimal* TEI command (`tests/docker-compose.test.yml` passes no
  truncation flags at all), never of the deploy command. Fixed:
  install.md and compose.env.example now say `cpu-1.9` (same TEI
  release as the CUDA `89-1.9` tag) and warn about pre-1.8 tags.

## The ksandbox first-install (#779)

Run 2026-07-29/30 from kubs0 via the kai ssh hop (ksandbox authorizes
only kai/cleo keys), following docs/install.md §1–§5 + §9 as a
stranger would: rustup + `cargo install just` + clone → provision →
CPU checklist → compose up → release build → `just smoke`. Log of
findings, each folded back into the docs in this sprint:

1. **Provision on a truly clean host works** — four files rendered,
   tokens matched to grants, operator token printed once. The #773
   fix holds where it had never been exercised before.
2. **`build-essential` missing** from install.md's prereq table —
   ksandbox (deliberately minimal Ubuntu server) has no C compiler,
   and several crates need `cc`. Added to the table.
3. **The `cpu-1.7` crashloop** (decision entry above) — the sprint's
   headline finding.
4. **Reranker on CPU**: reaches Ready but loads slowly (healthcheck
   shows *unhealthy* for minutes) and the CPU backend forces
   `max_batch_requests=4`. Noted in install.md; skipping it stays the
   CPU default.

Result: `just smoke` → **7 passed, 0 failed** on the empty ksandbox
store (fact round-trip 20 ms; knowledge write→embed→search 1 s), on
klams 0.1.35 built on the box. #779's acceptance — provision → boot →
printed token completes a memory_add round-trip — met on the CPU
branch kubs0 can never exercise.

The install was left quiesced and cleanly removable: service process
stopped, all four klams containers stopped (survives reboots), the
box's harness-eval tenants untouched. Footprint + exact resume/removal
commands: **k-homelab #787** (the record-machine-change note the WI
comment required).

## Deployed 2026-07-30

- Version `0.1.35` live on kubs0 (`/healthz` confirms; was `0.1.34`).
- Rollback target: `0.1.34` via `just rollback` (`.prev` binaries in
  place, rotated 2026-07-30).
- Migrations applied: none this sprint.
- Verified live: `just health` (2/2), **`just smoke`** — this sprint's
  own deliverable — 7 passed / 0 failed against the deployed service;
  the deployed `klams-scanner` binary refuses a nonexistent root with
  the new actionable error (#776 fail-loudly behavior). Units
  `klams-service` / `klams-monitor` / `klams-scanner.timer` all
  active; journal clean.
- Config changes required: none (`/etc/klams/*.toml` untouched; no new
  scopes or sections).

## Outcome

- `just gate` ✓, `just gate-viewport` ✓, `just test-integration` ✓
  (test stack swept + torn down after).
- `just smoke` green against the live kubs0 service (0.1.34) and the
  fresh ksandbox install (0.1.35).
- korg: #779 carries the run record; the proposal (korg:781) closes
  via sprint-ship.
