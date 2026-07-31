---
name: deploy-kubs0
description: Build the klams release binaries from a clean working tree and deploy them to the kubs0 systemd units (service + scanner + monitor). Use when asked to deploy/redeploy/ship klams. Runs locally on kubs0 — klams is built where it runs.
---

# Deploy klams to kubs0

Builds `klams-service`, `klams-scanner` and `klams-monitor` in release mode from
**committed code**, installs them into `/usr/local/bin` (rotating the previous
copy to `.prev`), and restarts the long-running units.

Unlike korg, there is no image and no SSH: klams is built on the host it runs
on. This skill is **local**.

## Deploys are clean-tree only

**Refuse to deploy from a dirty working tree.** `just install-systemd` compiles
whatever is in the tree, so uncommitted work would ship with nothing recording
what is actually running — `/healthz` would report a version that does not
correspond to any commit. Production only ever receives builds of committed
code.

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
| `/usr/local/bin/klams-{service,scanner,monitor}` | deployed binaries (`.prev` = rollback target) |
| `/etc/klams/klams.toml` | config — **not** in this repo; holds bearer tokens |
| `/var/lib/klams` | state |
| `/gratch/klams-backup` | nightly `postgres-<date>.dump` + `qdrant-<date>.snapshot` |

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

## Procedure

1. **Build + install.** Compiles all three binaries in release mode, rotates
   each `/usr/local/bin/<bin>` to `<bin>.prev`, installs unit files, then
   `daemon-reload` + `enable --now`. Takes several minutes — run it in the
   background and poll.
   ```bash
   just install-systemd
   ```

   **This step restarts the scanner timer, whether or not you wanted
   it.** The script ends with `systemctl enable --now
   klams-scanner.timer`, and `--now` starts a timer that was merely
   `stop`ped. So a scanner deliberately paused — e.g. to hold the corpus
   still for a before/after retrieval measurement (sprint 041) — comes
   back mid-deploy and scans. If a measurement depends on a frozen
   corpus, re-stop it *after* this step, or do not deploy mid-measure.

2. **Restart — this step is not optional.**
   ```bash
   just restart
   ```
   `install-systemd.sh` finishes with `systemctl enable --now`, which is a
   **no-op for a unit that is already running**. Without this restart the new
   binary sits on disk while the old process keeps serving, and `/healthz`
   still reports the old version — a deploy that looks successful and changed
   nothing. `just restart` covers `klams-service` and `klams-monitor`; the
   scanner is a timer-driven one-shot and picks the new binary up on its next
   fire.

3. **Verify the version actually moved.** This is the whole point of the
   PATCH-is-the-sprint-number convention:
   ```bash
   curl -s http://127.0.0.1:7777/healthz | jq '{version, status, postgres, qdrant, embeddings}'
   ```
   `version` must equal `Cargo.toml`'s, and it must differ from the preflight
   value. `status` should be `Ok` with all three backends `Ok`. If the version
   did not change, the restart did not take — do not proceed, and do not
   describe the deploy as done.

4. **Functional smoke.**
   ```bash
   just health     # light SC smoke
   just verify     # full SC-001..SC-009, slower
   ```
   Then smoke-test **what this sprint actually changed** — no fixed script can
   do that part. A new endpoint answers, a refused call is refused, a changed
   status code appears. `/healthz` alone proves only that a process is
   listening; a process running last month's binary passes it happily.

5. **Check the units settled**, not just that they started:
   ```bash
   systemctl is-active klams-service klams-monitor
   journalctl -u klams-service --since '2 min ago' --no-pager | tail -30
   ```
   `Restart=on-failure` means a crash-looping service can look briefly alive.

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

## Rollback

```bash
just rollback
```

Swaps every `/usr/local/bin/<bin>` with its `.prev` (moving the bad one to
`.broken`) and restarts the long-running units. No-op where no `.prev` exists —
so check preflight step 5 confirmed one is there *before* relying on this.

Then verify the version went back:

```bash
curl -s http://127.0.0.1:7777/healthz | jq .version
```

Only one generation is retained: `.prev` is overwritten on every deploy, so
this rolls back exactly one step. Two bad deploys in a row leave no binary
rollback path — rebuild from the last good tag instead.

## After a successful deploy

Record it in the sprint directory. **klams uses `sprints/<branch>/sprint.md`,
not `README.md`** — append a short section there rather than creating a new
file (this overrides `/sprint-ship` Phase 7.3's default filename):

```markdown
## Deployed <YYYY-MM-DD>

- Version `0.1.NN` live on kubs0 (`/healthz` confirms).
- Rollback target: `0.1.NN-1` via `just rollback` (`.prev` binaries in place).
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
