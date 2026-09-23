# Sprint 054 — a systemd unit owns the compose stack

Proposal korg:3042, **slice 1 of program korg:3044** (*klams' database
password gets a home, a rotation, and a safe maintenance window*), run
as an overseen karc leg (`klams-65a53f`) on kubs0. Covers one work
item: **#2711**. Version `0.1.54`.

## Goal

Ken's ruling of 2026-09-21 (korg:2711 comment 2816), as amended by the
overseer's readiness pass (korg:3042 notes, points 1–7):

- a **system** unit owns `docker compose up -d` / `down` for the four
  backing containers, with `EnvironmentFile=/etc/klams/compose.env`;
- `/etc/klams/compose.env`, root `0600`, beside `klams.toml` (which holds
  the same password as its connection URL), created **without reading a
  value into a transcript**;
- `${VAR:?}` guards on **all seven** interpolated variables, and
  `KLAMS_DATA_ROOT` matters most;
- `compose.env.example` stops describing a file the deploy doesn't use.

Klams must never actually stop during the slice.

## Premise check: drifted in mechanism, conclusion stands

Recorded on korg:3042 (comment 2934).

- **WI 2711's "nothing on disk reproduces the environment" was false.**
  The containers' compose labels
  (`com.docker.compose.project.environment_file`) name
  `/ai/klams/config/compose.env`, which is the `$KLAMS_ROOT/config/compose.env`
  that setup.md and install.md describe. It exists (ken:ai `0640`) and
  has the expected key names. #2711 had looked only in `deploy/`. The
  real hazard was any compose run that **omitted `--env-file`**:
  `just compose-up`/`-down`/`-rebuild`, or a bare `docker compose` in
  `deploy/`. The guards close exactly that.
- **Seven variables, `KLAMS_DATA_ROOT` without a default** (overseer
  point 3): confirmed. The header claimed a default that the compose
  never had.
- **Two more things the unit has to reproduce, or its first `up`
  recreates:**
  - `klams-tei` and `klams-reranker` were created with
    `docker-compose.gpu.yml` as well (label `config_files`).
  - The compose project is named **`deploy`** (the directory basename).
    `klams-postgres`/`klams-qdrant` date from compose 5.1.3 and the TEI
    pair from 5.2.0. The config hashes still matched (below).

### Overseer point 4, verified: an empty password is inert on this data dir

`postgres:16`'s `docker-entrypoint.sh` sets `DATABASE_ALREADY_EXISTS`
when `$PGDATA/PG_VERSION` is non-empty. `docker_verify_minimum_env` (the
empty-password refusal) and initdb both run **only** when that is unset.
`docker inspect` → Mounts: `klams-postgres` binds
`/ai/klams/data/postgres → /var/lib/postgresql/data` and `klams-qdrant`
binds `/ai/klams/data/qdrant`. So while `KLAMS_DATA_ROOT` resolves, the
container's `POSTGRES_PASSWORD` does nothing for auth: the role's
password lives in the cluster. **Slice 2 therefore needs no container
recreate to rotate.** It needs `ALTER ROLE`, `klams.toml`'s URL and
`/etc/klams/compose.env`, the last only so a future initdb (disaster
recovery) gets the right value.

## Decisions

- **`COMPOSE_FILE` in the env file, not a path in the unit.** The unit
  runs `docker compose up -d` from `/` and names no host path. The file's
  `COMPOSE_FILE` lists the compose file(s) by absolute path, GPU override
  included on a GPU host. The project name defaults to the first file's
  directory (`deploy`), which matches kubs0, so `COMPOSE_PROJECT_NAME`
  is documented but not set. This keeps the shipped unit generic
  (AGENTS.md portability line) and keeps the compose files where their
  relative `./prometheus` mounts resolve. The stack still runs from the
  repo working tree, as it always has.
- **Copied the existing env file rather than rebuilding it from
  `docker inspect`** (the overseer's point 5 suggested the latter). One
  root-side `install -m 0600` of the file that actually created the
  containers is simpler and reads nothing. Point 6's no-recreate proof
  checks either source equally.
- **`EnvironmentFile=` as ruled, not `--env-file`.** systemd's and
  compose's dotenv parsers could in principle disagree on a quoted
  value. The dry run under the unit's own environment
  (`systemd-run -p EnvironmentFile=…`) is what proves they didn't here.
- **`Wants=`/`After=docker.service`, not `Requires=`.** A docker package
  upgrade restarts `docker.service`, and `Requires=` would propagate
  that as a `down`+`up` of our unit, recreating every container. Restart
  policy already brings them back.
- **`Type=oneshot` + `RemainAfterExit`.** `restart` = down + up, which is
  the way to apply a changed env. The deploy skill now says a binary
  deploy never restarts it.
- **The installer always installs the unit and enables it only if
  `/etc/klams/compose.env` exists.** This mirrors the sprint 051 drop-in
  rule: a host that hasn't moved in keeps an installer that completes.
- **`check-compose` joins `just gate`**, and so CI. It renders offline
  (`docker compose config`, no daemon) in a scrubbed `env -i`, first
  with all seven set as a control, then blanks each variable in turn,
  empty and unset. It fails unless the render is refused **and** the
  error names that variable, because a refusal for another reason is not
  a pass. It was written first and failed 14/14 before the guards.

## Live install on kubs0: klams never stopped

All from kubs0, the host doing the work.

1. `sudo install -m 0600 -o root -g root /ai/klams/config/compose.env
   /etc/klams/compose.env`, then a checked trailing newline and an
   appended `COMPOSE_FILE=` line (both compose files). Key names were
   listed with `sed 's/=.*//'`; no value was printed.
2. **Dry run under the unit's environment**
   (`systemd-run --pipe -p EnvironmentFile=/etc/klams/compose.env docker
   compose --dry-run up -d`) reported all four `Running`, no `Recreate`.
3. **Control for step 2:** the same dry run with
   `RERANKER_MODEL_ID=control/bogus` reported `klams-reranker Recreate`.
   With the GPU file dropped from `COMPOSE_FILE`, it reported
   `Recreate` for `klams-tei` and `klams-reranker`. So the dry run can
   see a difference, and step 2's clean result means something.
4. Installed the unit (`systemd-analyze verify` clean), then
   `daemon-reload` and `enable --now`. The journal shows all four
   containers `Running`, `Finished`. **Container IDs before and after
   are identical** (`d169019061b7`, `f98f9ec14bd6`, `2ec5bcc06033`,
   `2e2c4e4ba7bf`), all still `Up 3 weeks (healthy)`. Nothing was
   recreated.
5. **Guard controls against the live environment:** with
   `KLAMS_DATA_ROOT=` and then `POSTGRES_PASSWORD=` blanked, each
   refused with `required variable … is missing a value` and rc=1. These
   ran as `--dry-run`, so a broken guard could not have recreated onto
   empty directories. A **real** bare `docker compose up -d` in
   `deploy/` (the #2711 hazard itself, run only after `check-compose`
   had proven the guards) refused on `POSTGRES_IMAGE_TAG`, rc=1. Same
   four container IDs afterwards.
6. `memory_search` answered live.

Only the unit steps of `install-systemd.sh` were run by hand. The full
script would also have swapped `/usr/local/bin` binaries in from
`target/release`, and the deploy skill installs those from the package
store. Its `--dry-run` shows the new step 5a rendering as intended.

## What shipped

- `deploy/klams-stack.service`: new system unit.
- `deploy/docker-compose.yml`: `:?` guards on all seven variables (13
  occurrences). The header no longer claims a default.
- `deploy/install-systemd.sh`: installs the unit and enables it only if
  its env file exists.
- `deploy/compose.env.example`: describes both homes (systemd and by
  hand), plus the commented `COMPOSE_FILE`.
- `scripts/check-compose-guards.sh` + `just check-compose`, added to
  `just gate`.
- Docs: setup.md (new section, including the move-in procedure and
  installer step 7), install.md §6, architecture.md §1.3/§4.2, usage.md
  recipe table, the deploy-kubs0 skill, and comments on the justfile
  compose recipes.

## Left for the overseer

- **`/ai/klams/config/compose.env` is now a stale second copy of the
  password**, group-`ai`-readable and read by nothing. Removing it is the
  point of Ken's one-directory decision, and slice 2's rotation would
  otherwise leave the old value there. It is **not deleted**, because
  deleting a secret-bearing file is irreversible. The recommendation is
  to remove it, or to make that step 0 of slice 2.
- **krot's registry text is stale**: the klams row
  (`kai:~/src/tools/krot/registry/klams.toml`, surfaced by
  `memory_search`) still says "RECREATE TRAP … NOT yet fixed … no
  compose.env". That belongs to krot and to slice 2, which edits that row
  anyway.

## Deployed 2026-09-23

- Version `0.1.54` live on kubs0: `/healthz` reports `Ok` with all four
  backends `Ok` (it was `0.1.52`). `klams-token`, `klams-scanner` and
  `klams-monitor --version` all report `0.1.54`.
- Published to the store as `artifacts/klams-*/0.1.54/`, via `just publish`
  run in the foreground. A first background attempt was killed with the
  ship turn's unit (signal 15) before it published anything; the store
  still read `0.1.52` for all four.
- Installed with `just deploy-from-store`, then `just restart`
  (klams-service, klams-monitor).
- Unit files: `klams-stack.service` was already installed during the
  sprint, byte-identical to the repo (`cmp`), so `install-systemd` was not
  run. **`klams-stack` was not restarted**, and container IDs are
  unchanged across the deploy (`d169019061b7 f98f9ec14bd6 2ec5bcc06033
  2e2c4e4ba7bf`). `systemctl is-active klams-stack klams-service
  klams-monitor` reports all three `active`.
- Backup timing: the overseer asked to wait for "tonight's 01:01 UTC" pair.
  The scheduler's window is actually **08:01 UTC** (01:01 PDT; the backup
  listing shows local time). The 2026-09-22 pair was that night's run,
  logged `backup run complete ok` at 08:01:15 UTC, about 17 hours before
  the deploy. The next window is 2026-09-23 08:01 UTC, so the restart could
  not collide with a backup, and the deploy went ahead.
- kai's `klams-scanner`: left at `0.1.52`. Nothing in this sprint touches
  the scanner.
- Rollback target: `0.1.52` via `just rollback` (the `.prev` binaries are
  in place), or `just deploy-from-store --version 0.1.52`.
- Migrations applied: none.
- Verified live: `KLAMS_AGENT=claude just health` passed 2, failed 0. An
  earlier run as `operator` got a 401, because kubs0 has no such identity.
  `memory_search` answered. No ERROR/WARN in klams-service's journal after
  the restart.
- Config changes required: none.
