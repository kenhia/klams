# Sprint 013 — Pivot from spec-kit

**Branch:** `chore-pivot-from-speckit` (predates the branch-name
convention this sprint introduces; from 014 on, branch =
`###-<short-stub>` matching the sprint directory).
**Type:** chore — no runtime behavior change.
**Context:** korg WI #259 (k-homelab). The keep-vs-greenfield review
([planning/wi259-recommendation.md](../planning/wi259-recommendation.md))
landed on keeping klams; this sprint retires the spec-kit scaffolding
in favor of a lighter sprint workflow, since PR-sized personal sprints
don't need the full spec/plan/tasks/checklist ceremony.

## Goal & acceptance

Remove spec-kit from the repo without losing (a) the constitution's
principles or (b) any load-bearing path. Done when:

- `specs/` is renamed to `sprints/`, and no tracked file references the
  old path (external `ansible-k/specs/...` links deliberately excluded).
- Spec-kit tooling is gone: `.specify/`, `.github/prompts/speckit.*`,
  `.github/agents/speckit.*`, speckit entries in `.vscode/settings.json`.
- The constitution's transferable principles live in a root
  `AGENTS.md` consumed by both Claude Code (via `CLAUDE.md` import)
  and GHCP (native `AGENTS.md` support + pointer in
  `.github/copilot-instructions.md`).
- The new sprint workflow (branch + `sprints/###-<stub>/` dir,
  markdown chronicle) is documented in `AGENTS.md`.
- `just gate` passes.

## What was done

1. `git mv specs sprints`; copied the two WI #259 analysis docs into
   `sprints/planning/`.
2. Removed `.specify/` (34 files), `.github/prompts/` and
   `.github/agents/` (28 speckit files), and the
   `chat.promptFilesRecommendations` / `.specify` auto-approve blocks
   from `.vscode/settings.json`.
3. Repo-wide `specs/` → `sprints/` rewrite with negative lookbehinds
   for `ansible-k/` and `blob/main/` so references to the *external*
   ansible-k repo's `specs/klams-integration/` stay intact.
   Load-bearing (non-comment) fixes included:
   - `.gitignore` (sizing.md / dashboard-smoke.md ignore rules)
   - `justfile` `backup-size` recipe output path
   - `tools/bench/src/bin/run.rs` default perf-baseline output path
   - `crates/klams-service/tests/backup_status_hook_schema.rs`
     (schema fixture path)
   - `crates/klams-service/tests/us3e_handoff_layout.rs` — both the
     `.join("specs")` path segment and the pinned-version header
     string, which was rewritten in lockstep with
     `sprints/003-non-agentic-writes/handoff/README.md` so the
     verbatim-match test still passes.
4. Authored `AGENTS.md` (constitution capture + sprint workflow),
   `CLAUDE.md` (`@AGENTS.md` import), and replaced the speckit-managed
   `.github/copilot-instructions.md` with a pointer to `AGENTS.md`.
   README's constitution link now points at `AGENTS.md`.

## Notes & decisions

- Sprints 001–012 keep their spec-kit internal layout; only the parent
  directory name changed. No retrofit.
- The pinned handoff copy in `~/ansible-k/specs/klams-integration/`
  still cites the old `specs/003-...` path of this repo. Harmless
  (it's a historical pin), but worth a one-line touch-up next time
  that handoff is edited.
- `.github/memories/` (GHCP memory primers) is unrelated to spec-kit
  and was kept as-is.
