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
