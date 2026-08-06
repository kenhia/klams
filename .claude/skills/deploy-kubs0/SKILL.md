---
name: deploy-kubs0
description: Publish the klams release binaries to the homelab package store from a clean working tree, then install them onto the kubs0 systemd units (service + scanner + monitor) and restart. Use when asked to deploy/redeploy/ship klams. Runs locally on kubs0 — klams is built where it runs.
---

# Deploy klams to kubs0

Builds `klams-service`, `klams-scanner` and `klams-monitor` in release mode from
**committed code**, publishes them to the homelab package store, installs the
published binaries into `/usr/local/bin` (rotating the previous copy to
`.prev`), and restarts the long-running units.

Unlike korg, there is no image and no SSH *to build*: klams is compiled on the
host it runs on. This skill is **local**.

## Deploys go through the store (sprint 042, #1012)

Since sprint 042 the deploy is two steps — **publish, then install** — rather
than one build-in-place. It is worth knowing why, because the extra step looks
like ceremony on the host that does the building:

- kubs0 builds klams, but it is not the only host that *runs* it.
  `klams-scanner` also runs on kai, which has no checkout and no Rust
  toolchain. Publishing is what makes one release reachable from both. Before
  it, kai sat 13 releases behind for months.
- `.prev` is one slot deep. The store keeps every published version, so
  rollback stops being "whatever this host happened to replace last".
- Homelab doctrine — k-homelab `docs/deploying.md`: *every deploy publishes a
  versioned asset to the store; every install and deploy pulls from the store,
  even when the deploy is local.*

`just install-systemd` still exists and still builds in place. It is now the
**unit-file** path, not the ordinary deploy path — see "When you still need
`install-systemd`" below.

## Deploys are clean-tree only

**Refuse to deploy from a dirty working tree.** Uncommitted work would ship
with nothing recording what is actually running — `/healthz` would report a
version that does not correspond to any commit. Production only ever receives
builds of committed code.

`just publish` enforces this itself (it refuses a dirty tree, untracked files
included), but check in preflight anyway so you fail before the release
compile rather than after it.

Never stash to get around this. Report what is dirty and ask.

## Target

`kubs0` — the host this repo lives on. Three native systemd units, with
Postgres / Qdrant / TEI as Docker containers underneath (hence
`After=/Wants=docker.service`).

| Unit | Role |
|---|---|
| `klams-service.service` | REST + MCP on `:7777`; long-running |
| `klams-monitor.service` | host/service event emitter; long-running |
| `klams-scanner.timer` → `klams-scanner.service` | periodic filesystem scan (one-shot, ~hourly) |

| Path | What |
|---|---|
| `/usr/local/bin/klams-{service,scanner,monitor}` | deployed binaries (`.prev` = one-step rollback) |
| `/etc/klams/klams.toml` | config — **not** in this repo; holds bearer tokens |
| `/var/lib/klams` | state |
| `/gratch/klams-backup` | nightly `postgres-<date>.dump` + `qdrant-<date>.snapshot` |
| `$KLAMS_STORE_URL/artifacts/klams-*/` | published releases (the deep rollback) |

**This skill must run on kubs0.** Confirm with `hostname` before anything else.
If you are on kai (or anywhere else), stop — the working copy and the build
toolchain for the deployed artifact live on kubs0. Do not attempt a remote
build.

## Preflight

1. **Clean tree — stop here if it is not.**
   ```bash
   git status --porcelain
   ```
   Any output means **do not deploy**. Untracked files count: they may be
   sources the build would pick up.

2. **Right host.**
   ```bash
   hostname   # must be kubs0
   ```

3. **Gate is green** — `just gate` (fmt + clippy + tests), or confirm the
   caller already ran it. A broken build wastes a multi-minute release compile.
   `/sprint-ship` has already done this by Phase 7.

4. **Backups current.** A restart applies pending schema migrations
   automatically (see below), and last night's pair is what makes a bad one
   survivable.
   ```bash
   sudo ls -la /gratch/klams-backup/ | tail -4
   ```
   Expect a `postgres-<UTC-date>.dump` and `qdrant-<UTC-date>.snapshot` from
   last night, with the qdrant snapshot no smaller than the previous one. If
   stale or shrinking, say so and ask before continuing.

5. **Record the rollback target** — the version now live and the binary
   timestamps:
   ```bash
   curl -s http://127.0.0.1:7777/healthz | jq '{version, uptime_seconds}'
   ls -la /usr/local/bin/klams-service /usr/local/bin/klams-service.prev
   ```
   Note the reported `version`. That is what you roll back *to*, and the number
   the post-deploy check must no longer see.

6. **Know the expected version.** The PATCH segment of `[workspace.package]
   version` in `Cargo.toml` is the sprint number (AGENTS.md). Read it — it is
   the deploy's success criterion:
   ```bash
   grep '^version' Cargo.toml
   ```

7. **Store variables are set.** Both have **no default** on purpose — unset,
   the recipes stop and name the variable rather than guessing a hostname.
   They belong in the gitignored `.env` at the repo root:
   ```bash
   grep -c 'KLAMS_STORE_' .env      # expect 2; values are in .env, not here
   ```
   | Variable | Used by |
   |---|---|
   | `KLAMS_STORE_HOST` | `just publish` — the ssh host running `kpkg` |
   | `KLAMS_STORE_URL` | `just deploy-from-store` — the store's base URL |

8. **This version is not already published.** Published versions are
   immutable; `kpkg` refuses to overwrite one.
   ```bash
   curl -fsS "$KLAMS_STORE_URL/artifacts/klams-service/latest"
   ```
   If it already reads the version you are about to ship, do **not** reach for
   `--force`. Either the published build is the code you intend to deploy — in
   which case skip step 1 and go straight to step 2, saying so in the deploy
   record — or it is not, and the version needs bumping.

## Procedure

1. **Build + publish.** Compiles all three binaries in release mode and
   publishes each under its own artifact name. Takes several minutes — run it
   in the background and poll.
   ```bash
   just publish
   ```

   It refuses two things, and both refusals are the recipe working:

   - **a dirty tree** — a published version must name a commit;
   - **binaries that disagree about `--version`** — the installer asserts the
     same invariant on the way in, so a mismatch is caught before it reaches a
     host rather than after.

2. **Install from the store.** Fetches, checksum-verifies, asserts each binary
   reports the version it was published under, then rotates
   `/usr/local/bin/<bin>` to `<bin>.prev` and installs.
   ```bash
   just deploy-from-store
   ```

   Verification is two-layer and worth understanding: the SHA256 proves the
   *transfer*, the `--version` assertion proves the *label*. A mislabelled
   publish is invisible to a checksum and would silently defeat k-homelab's
   version floor, which is the only drift alarm the fleet has.

   **Every binary is verified before any is installed.** A failure on the
   third one leaves the first two untouched — there is no half-applied state
   to reason about.

   Unlike `install-systemd`, this **touches no unit files and restarts
   nothing**. That is why a deliberately paused scanner timer survives a
   deploy now (see the note under `install-systemd` below).

3. **Restart — this step is not optional.**
   ```bash
   just restart
   ```
   `deploy-from-store` installs and stops; it prints what to activate and
   leaves the decision to you. Without this restart the new binary sits on
   disk while the old process keeps serving, and `/healthz` still reports the
   old version — a deploy that looks successful and changed nothing.
   `just restart` covers `klams-service` and `klams-monitor`; the scanner is a
   timer-driven one-shot and picks the new binary up on its next fire (force
   one with `sudo systemctl start klams-scanner.service`).

4. **Verify the version actually moved.** This is the whole point of the
   PATCH-is-the-sprint-number convention:
   ```bash
   curl -s http://127.0.0.1:7777/healthz | jq '{version, status, postgres, qdrant, embeddings, reranker}'
   ```
   `version` must equal `Cargo.toml`'s, and it must differ from the preflight
   value. `status` should be `Ok` with all three backends `Ok`. If the version
   did not change, the restart did not take — do not proceed, and do not
   describe the deploy as done.

5. **Functional smoke.**
   ```bash
   just health     # light SC smoke
   just verify     # full SC-001..SC-009, slower
   ```
   Then smoke-test **what this sprint actually changed** — no fixed script can
   do that part. A new endpoint answers, a refused call is refused, a changed
   status code appears. `/healthz` alone proves only that a process is
   listening; a process running last month's binary passes it happily.

6. **Check the units settled**, not just that they started:
   ```bash
   systemctl is-active klams-service klams-monitor
   journalctl -u klams-service --since '2 min ago' --no-pager | tail -30
   ```
   `Restart=on-failure` means a crash-looping service can look briefly alive.

   A single `klams-monitor` `publish failed … POST /memory/events` right at
   restart is **expected and not a regression**: the monitor sees the unit go
   active a couple of hundred milliseconds before the service binds `:7777`,
   and publishes into a closed port. Count them rather than reacting to one —
   a handful at startup that then stop is the known shape.

## The same release reaches kai

`klams-scanner` also runs on kai, which the version floor in k-homelab's
`recipes/klams-scanner` asserts against. Once a version is published, kai
takes it with:

```bash
just deploy-remote kai klams-scanner
```

kai needs no checkout and no Rust toolchain — it fetches the installer out of
the store and verifies it against the same `SHA256SUMS` as the binary. It is
not part of the kubs0 deploy, so **decide explicitly whether this ship should
include it**, and say which way in the deploy record. A scanner-affecting
sprint that leaves kai behind is how the drift in #836 accumulated in the
first place.

Confirm with `ssh -n kai klams-scanner --version`, and — since it is the
check that actually watches for drift — `~/k-homelab/bin/audit kai`.

## When you still need `install-systemd`

`just install-systemd` builds in place and is the **only** path that installs
unit files. Use it when the sprint changed anything in `deploy/*.service` or
`deploy/*.timer`, or on a host being provisioned for the first time (it also
creates the `klams` user and `/var/lib/klams`, `/etc/klams`).

Check whether that applies before assuming it does not:

```bash
git diff --name-only <last-deployed-ref>..HEAD -- deploy/
```

When it does, run `install-systemd` **as well as** the store install, not
instead of it — publish first so the store still holds the release, and so
kai can reach it.

**`install-systemd` restarts the scanner timer whether or not you wanted it.**
The script ends with `systemctl enable --now klams-scanner.timer`, and
`--now` starts a timer that was merely `stop`ped. A scanner deliberately
paused — e.g. to hold the corpus still for a before/after retrieval
measurement (sprint 041) — comes back mid-deploy and scans. If a measurement
depends on a frozen corpus, re-stop it *after* this step, or do not deploy
mid-measure. `deploy-from-store` has no such hazard: it touches no units.

## Config changes are a separate, manual step

`/etc/klams/klams.toml` is **not** in this repo and is not touched by this
skill. It holds the bearer tokens. If the sprint changes the config contract —
a new `[[auth.tokens]]` scope, a new section — say so explicitly and let Ken
edit it; never write tokens into the repo or paste them into a transcript.

`[[auth.tokens]]` hot-reloads without a restart:

```bash
sudo systemctl reload klams-service     # ExecReload=/bin/kill -HUP $MAINPID
```

When a sprint's correctness depends on a config change (e.g. a grant needing a
newly added scope), verify the *effect* after reloading, not just that reload
returned.

## Migrations run at startup

`PostgresStore::connect` applies pending `sqlx` migrations as a side effect, so
**a restart migrates**. Consequences:

- Migrations need no separate step. `just db-migrate` exists to apply them
  *without* starting the server, which is useful for a pre-flight dry run.
- **Rollback is binary-only and does not undo a migration.** Going back across
  a migration boundary needs a restore from a dump
  (`just restore-from <date>`), not a binary swap. If the sprint added
  migrations, note that in the deploy record.
- If a sprint includes a data migration, capture row counts before and diff
  after. A migration that silently drops rows looks identical to a healthy
  deploy from outside.

## Rollback — two depths

**One step back, fastest:**

```bash
just rollback
```

Swaps every `/usr/local/bin/<bin>` with its `.prev` (moving the bad one to
`.broken`) and restarts the long-running units. No-op where no `.prev` exists —
so check preflight step 5 confirmed one is there *before* relying on this.

Only one generation is retained: `.prev` is overwritten on every deploy, so
this rolls back exactly one step.

**Any published version, no rebuild** (sprint 042 — this is what the store
bought):

```bash
just deploy-from-store --version 0.1.NN
just restart
```

Two bad deploys in a row no longer leave you without a path: every version
ever published is still there, and it is the *artifact that ran*, not a fresh
build of old source on today's toolchain. List what is available with
`ssh $KLAMS_STORE_HOST kpkg list`.

Either way, verify the version went back:

```bash
curl -s http://127.0.0.1:7777/healthz | jq .version
```

**Neither undoes a migration** — see above. A rollback across a migration
boundary needs `just restore-from <date>`.

## After a successful deploy

Record it in the sprint directory. **klams uses `sprints/<branch>/sprint.md`,
not `README.md`** — append a short section there rather than creating a new
file (this overrides `/sprint-ship` Phase 7.3's default filename):

```markdown
## Deployed <YYYY-MM-DD>

- Version `0.1.NN` live on kubs0 (`/healthz` confirms; was `0.1.NN-1`).
- Published to the store as `artifacts/klams-*/0.1.NN/`.
- Unit files: <unchanged, so `install-systemd` not run | changed, so it was>.
- kai's `klams-scanner`: <deployed to 0.1.NN | left at 0.1.NN, and why>.
- Rollback target: `0.1.NN-1` via `just rollback` (`.prev` binaries in place);
  any published version via `just deploy-from-store --version`.
- Migrations applied: <none | list>.
- Verified live: <what you actually exercised, beyond /healthz>.
- Config changes required: <none | what Ken changed in /etc/klams/klams.toml>.
```

The feature branch is gone by Phase 7, so commit this to `main` directly.
**Put `[skip ci]` in the subject** — this is load-bearing, not cosmetic:

```
docs(NNN): record the deploy [skip ci]
```

Sprint 040 (#792): without it, this second push to `main` starts a run
that cancels the merge commit's still-in-flight one, so the commit
actually carrying the sprint's code never completes a verdict. That
happened on every ship from at least 033 through 039. It also meant the
main-only `perf` job (~7 min, cancelled at ~2) only ever measured the
docs commit, never the code.

GitHub honours `[skip ci]` / `[ci skip]` / `[no ci]` natively on push —
the workflow is skipped before any job is evaluated, so nothing in
`ci.yml` needs to know about it. `ci.yml` also scopes
`cancel-in-progress` to non-`main` refs as a backstop, but that only
limits the damage; this is what prevents it. A deploy record changes no
code, so skipping CI for it costs no coverage.
