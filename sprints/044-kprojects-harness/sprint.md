# Sprint 044 — klams onto the kprojects harness

**korg:** proposal 1242 (program 1245, "kprojects rollout, batch 2 —
klams and kvscf") · work item #1237 · chore / S
**Branch:** `044-kprojects-harness` · **Version:** 0.1.44

## Goal

Put this repo on the [kprojects](https://github.com/kenhia/kprojects)
minimal harness: the shared-conventions managed block in the agent
instruction files, the harness directory skeleton, and the stack's
`.gitignore` stanza. klams is a *skills-only* repo today — it never
carried Spec-Kit machinery (that was retired back in sprint 013) — so
this is an additive conversion with nothing to collapse.

Batch 2 of the rollout deliberately moves off kai to prove the flow on
the other two hosts. klams is the kubs0 slice; kvscf (cleo, Windows) is
the other and is a separate branch and PR.

## Scope

In scope — the harness's own files only:

- Run `kproject-install --agent both` against this checkout, with
  **no `--stack`**: the root `Cargo.toml` is first in the detection
  order, so `rust` is unambiguous. Confirm the reported
  `stack : rust (detected)` line anyway — the tooling stanza inside the
  managed block cannot be hand-corrected, only re-applied with an
  explicit `--stack`.
- Managed block (`kproject:begin` / `kproject:end`) in `CLAUDE.md` and
  `.github/copilot-instructions.md`, both of which are currently
  pointers at `AGENTS.md`. The project-specific section lives *outside*
  the block and keeps pointing at `AGENTS.md`, which stays the working
  agreement.
- **Extend, do not replace, the justfile.** klams' `gate` recipe is what
  has kept it green since 013 and is what CI invokes (sprint 040, #788).
  If the installer would seed a `check`, `check` becomes an alias for
  `gate` — one definition, no second gate to drift.
- Harness skeleton: `sprints/planning` and `sprints/review`, `docs/`,
  `.scratch/`, plus the `.scratch/` and `.env` `.gitignore` entries.
  Most of these already exist; the install is idempotent.

Out of scope, explicitly:

- **The running service.** klams runs live on kubs0 and is the shared
  memory every agent on the fleet calls, over MCP and HTTP. This is a
  repo-layout change: no restart, no redeploy, no systemd, token,
  Postgres or Qdrant changes.
- **The other two clones.** `cleo:D:\src\klams` (dirty) and
  `kubs0:~/tmp-clone/klams` (stale) are on korg #737's duplicate-clone
  list and are resolved separately. This work happens in the kubs0
  primary, `~/src/ai/klams`.
- Any edit outside the harness's own files.

## Acceptance criteria

1. The managed block is present in both agent files, between the
   markers, unedited inside.
2. The installer reported `rust`, and the tooling stanza matches.
3. `just gate` passes, unchanged in what it runs; `just check` resolves
   to it rather than to a second, weaker gate.
4. `.gitignore` carries `.scratch/` and `.env` (both already did).
5. `AGENTS.md` remains the working agreement; nothing in it is
   contradicted by the managed block.
6. Nothing about the deployed service changed.

## Standing authorization

Ken, 2026-08-12 (recorded on korg #1237 and the program): this is a
chore sprint — creating the PR, merging it, and resetting local to main
proceed without further approval. Stop only if the migration would
change behaviour: a seeded gate that fails, an existing recipe that
would be overwritten, or an edit needed outside the harness's own files.

## What actually happened

Additive, as expected — no old-harness machinery to remove (Spec-Kit
left in 013), so nothing was deleted.

`kproject-install --agent both .` reported:

```
stack    : rust (detected)
layout   : sprints/planning sprints/review docs .scratch
gitignore: added target/
agents   : appended managed block in CLAUDE.md
agents   : appended managed block in .github/copilot-instructions.md
```

`rust (detected)` is the reading we wanted, so no `--stack` re-apply was
needed. Re-running the installer is genuinely idempotent: the second run
reported `updated managed block` for both files and added no second
block and no second `target/` line.

Three things the install left to decide, and how they were decided:

**1. `just check` vs `just gate`.** The managed block tells every agent
that `just check` runs the CI gates. klams has had `gate` since 013, and
since sprint 040 (#788) CI invokes `just gate` *by name* — so the recipe
is the single definition of the gate. The installer seeds a justfile
only when one is missing, so it wrote nothing here and `check` simply
did not exist. Resolved by adding `check: gate` — an alias, not a second
recipe. Two names, one gate, nothing that can drift, and CI is untouched.
`AGENTS.md` records the alias next to the gate it aliases.

**2. `sprints/review/` vs `docs/reviews/`.** The harness reserves
`sprints/review/` for formal reviews; klams already keeps its two (the
2026-07-25 deep review, the 2026-07-28 retrospective) in `docs/reviews/`,
cited by eight sprint records. Moving them would break the record they
are part of, so they stayed put and `sprints/review/README.md` points at
them — which also gives the otherwise-empty directory something to
commit, so the skeleton is actually in the repo rather than only on disk.

**3. `target/` in `.gitignore`.** Redundant — `/target/` and
`**/target/` were both already there. Left in place: it is what makes
the installer's `.gitignore` step a no-op on re-run, and removing it
would just mean the next run re-adds it.

The agent files were pointers at `AGENTS.md` before and still are; the
block is appended *below* that pointer, so `AGENTS.md` remains the
working agreement and the project-specific content lives outside the
managed block as the harness intends.

Version bumped 0.1.43 → 0.1.44 per the sprint convention (`Cargo.lock`
moves with it). Nothing was deployed: `/healthz` will keep reporting
0.1.43 until the next real deploy, which is correct — this sprint
changed no binary.

### Acceptance

1. ✅ Managed block present in both agent files, between markers,
   unedited inside (verified by the clean idempotent re-run).
2. ✅ `rust (detected)`.
3. ✅ `just check` passes and resolves to `gate`; `gate` itself is
   byte-identical to what CI has been running.
4. ✅ `.gitignore` carries `.scratch/` and `.env` (both predated this).
5. ✅ `AGENTS.md` unchanged except for the `check`-alias note.
6. ✅ Service untouched — no restart, redeploy, systemd, token, Postgres
   or Qdrant change. Work done in the kubs0 primary only.
