# Sprint 042 — klams deploys from the package store

**Proposal:** korg:1022 (covers #1012)
**Started:** 2026-08-05 · **Version:** 0.1.42
**Type:** deploy path. No behaviour change in the service.

Slice 2 of the cross-project **"Deploy from the store — fleet adoption"**
program (korg:1026). Slice 1 (korg → docker registry) landed 2026-08-06.

## Goal

Give klams a release path that does not require a checkout on the
consuming host. `just publish` puts versioned binaries in the homelab
package store; a self-contained installer fetches, checksum-verifies and
installs them on any tailnet host.

The concrete thing this closes: **kai has no klams checkout**, so there
has never been a way to update its `klams-scanner`.

## The live baseline

Measured on 2026-08-05, before any change:

| | |
|---|---|
| `kai:/usr/local/bin/klams-scanner --version` | **0.1.28** |
| kubs0 / klams `main` | **0.1.41** — 13 releases ahead |
| `ls -d kai:~/src/ai/klams` | **absent** |
| k-homelab `klams_scanner.min_version` (kai manifest) | `0.1.41` |
| `https://kubsdb.encke-wahoo.ts.net:4880/artifacts/` | reachable from kai; contains `kaed/` only |

k-homelab's `recipes/klams-scanner/apply.sh` already reports this, and
says so in as many words:

> `advisory: klams-scanner 0.1.28 is older than the declared floor 0.1.41
> (fix: deploy from the klams repo — **there is no automated path to
> kai**, klams #836 …)`

That parenthesis is the sprint. The recipe is correct and does not
change; what changes is that its fix clause becomes a command someone
can actually run.

## Why the binary and not the source

k-homelab `docs/deploying.md` settles this: the deploy asset for a
compiled service is the **binary**. kai *could* grow a checkout and a
Rust toolchain, but that recreates exactly the pinned-clone failure the
store exists to remove — two clones of one repo, nothing announcing
which one is stale. kubsdb has no cargo at all, so a
`cargo install --registry` shape was never available fleet-wide either.

## Scope decision — publish all three binaries, not just the scanner

WI #1012 names `artifacts/klams-scanner/`. The publish recipe also
covers `klams-service` and `klams-monitor`, for three reasons, at the
cost of two extra lines:

1. The doctrine is unconditional — *"every deploy publishes a versioned
   asset to the store; every install and deploy pulls from the store —
   even when the deploy is local."* kubs0's own deploy is a local deploy.
2. Rollback stops being a single `.prev` slot and becomes the store's
   version history. `.prev` still exists as the fast path; it is no
   longer the only one.
3. klams-view (#1013) is queued to copy this shape, and a shape that
   only covers a one-shot timer binary would not transfer.

The scanner remains the acceptance criterion: kai above the floor.

## Design

### One artifact name per binary

```
artifacts/klams-scanner/0.1.42/klams-scanner-x86_64-linux
artifacts/klams-scanner/0.1.42/install-from-store.sh
artifacts/klams-scanner/0.1.42/SHA256SUMS
artifacts/klams-scanner/latest                     → "0.1.42"
artifacts/klams-service/0.1.42/…
artifacts/klams-monitor/0.1.42/…
```

Per-binary names rather than one `klams` bundle, so a host fetches
exactly what it runs. kai wants the scanner and nothing else, and its
`latest` pointer should not move because the service was re-released.

The installer script is published **inside each artifact directory**, so
every directory is self-sufficient and the script is covered by the same
`SHA256SUMS` as the binary it installs. That is what makes the bootstrap
on a repo-less host a verified fetch rather than a `curl | bash`.

### `deploy/install-from-store.sh` — binaries only, never units

The installer does **not** touch unit files, config, or running
services. This is not tidiness; it is required. kai's
`klams-scanner.service` diverges from the repo's copy deliberately and
correctly (`User=ken`, no `After=klams-service.service` — there is no
local service on kai), and k-homelab's recipe README says not to "fix"
it. An installer that shipped units would overwrite that divergence on
every deploy.

Post-condition, not just a checksum: the script runs the freshly fetched
binary's `--version` and asserts it reports the version that was
requested. A checksum proves the transfer; this proves the *label*.

### Nothing Ken-shaped ships as a default

Per AGENTS.md, the store URL is `KLAMS_STORE_URL` and the publish host
is `KLAMS_STORE_HOST`, both with **no default** — the #682/#776 pattern.
Unset, the recipes stop and say which variable to set rather than
guessing a hostname. Ken's values live in the gitignored `.env`.

## Acceptance criteria

1. `just publish` puts 0.1.42 in the store under all three artifact names.
2. A host with no klams checkout can install the scanner from the store
   in one documented bootstrap.
3. `kai:klams-scanner --version` reports 0.1.42, above the 0.1.41 floor.
4. k-homelab `bin/audit` no longer emits the version advisory for kai.
5. kai's unit files and `scanner.toml` are byte-identical afterwards.

## Testing

The new code is mostly shell, which this repo already knows how to
test: `install_systemd_dry_run.rs` drives `install-systemd.sh` from
`cargo test`. Same idiom here, plus one thing that makes it cheap —
**`curl` speaks `file://`**, so the whole package store is a temp
directory and the happy path is exercised for real in `just gate`, with
no network and no server.

`crates/klams-service/tests/install_from_store.rs` (9 tests) publishes
stub binaries into a fake store and asserts:

- `latest` resolution, install, and the printed no-restart notice;
- `.prev` holds the version that was replaced;
- `--version <older>` pins the fetch (the rollback path);
- a **tampered** artifact is refused, and nothing lands;
- a **mislabelled** artifact is refused — the binary reports 0.0.1 while
  published as 9.9.9, which no checksum can catch and which would
  silently defeat k-homelab's version floor;
- one bad binary in a set installs *none* of them;
- an unset `KLAMS_STORE_URL` names the variable rather than failing as a
  curl error.

`crates/klams-service/tests/version_flag.rs` (3 tests) covers the
`klams-service --version` early-out below.

### Two defects the tests found before any host did

1. **`klams-service --version` did not exist.** It parses flags by hand
   (klams-scanner and klams-monitor use clap), and `resolve_config_path()`
   ran first — so on a host with no config it exited non-zero with a
   config error instead of printing a version. The fleet's freshness
   signal was unreadable on exactly the hosts being provisioned. Fixed
   with an early-out before config resolution, matching clap's
   `<name> <version>` output and its `-V` short form so the existing
   `awk '{print $NF}'` readers keep working.
2. **The installer demanded root when it needed a writable directory.**
   `[ "$(id -u)" -ne 0 ]` is a proxy for the real precondition, and it
   made `BIN_DST_DIR` (already overridable) useless — including for a
   host installing into `~/.local/bin`. Now checks `-d` and `-w` on the
   destination and names the user in the error.

## Outcome

See [outcome.md](outcome.md).

## Deployed 2026-08-06

- Version `0.1.42` live on kubs0 (`/healthz` confirms; was `0.1.41`,
  uptime 515177s). All four subsystems `Ok` — postgres, qdrant,
  embeddings, reranker.
- **Deployed via `just deploy-from-store`, not `just install-systemd`** —
  this sprint's own path, dogfooded on its own ship. All three binaries
  fetched from `artifacts/klams-*/0.1.42/`, checksum-verified, and
  version-asserted before any was swapped in.
  - The published 0.1.42 was built from `209ec98` (the branch tip before
    two docs commits), not from merged `main`. Checked rather than
    assumed: the only delta is `docs/architecture.md` and
    `sprints/042-scanner-from-store/outcome.md`, and no crate uses
    `include_str!`/`include_bytes!`, so nothing markdown reaches a
    binary. No unit files changed either, which is why skipping
    `install-systemd` was safe this time.
  - From sprint 043 the ordering should be the plain one: merge, then
    `just publish` from `main`, then `just deploy-from-store`.
- Rollback target: `0.1.41` via `just rollback` (`.prev` in place for all
  three). Deeper: `just deploy-from-store --version <older>`.
- Migrations applied: **none** — `migrations/` unchanged this sprint.
- Config changes required: **none**. `/etc/klams/klams.toml` untouched.
- The scanner timer was **not** disturbed: `install-systemd`'s
  `enable --now` (which restarts a paused timer whether you wanted it or
  not) never ran. Timer was `enabled` + `active` before and after.

### Verified live, beyond `/healthz`

- `just health` — 2 passed, 0 failed.
- `just verify` — SC-001..SC-009, 7 passed, 0 failed, 3 skipped
  (perf/restart/retired-UI, as designed).
- **The defect this sprint fixed, proved on the deployed binaries:**
  ```
  $ env -u KLAMS_CONFIG /usr/local/bin/klams-service --version
  klams-service 0.1.42                                    # exit 0
  $ env -u KLAMS_CONFIG /usr/local/bin/klams-service.prev --version
  Error: no config found: tried /ai/klams/config/klams.toml and …  # exit 1
  ```
  `-V` matches. The 0.1.41 binary now sitting in `.prev` is the contrast.
- The deploy log caught the same defect one last time on its way out:
  `rotating /usr/local/bin/klams-service (unknown) -> ….prev` — the
  installer could not read the outgoing binary's version, because that
  binary was the broken one. Every future rotation names both versions.
- All three deployed binaries report `0.1.42`; all three `.prev` report
  `0.1.41` (scanner and monitor — service's `.prev` cannot report at all,
  see above).
- Forced `systemctl start klams-scanner.service` on the new binary:
  started, scanned, `Deactivated successfully`, `is-failed` → `inactive`.

### One WARN, pre-existing and not a regression

`klams-monitor` logged one `publish failed … POST /memory/events` at
`04:46:18.670`. The service did not bind `127.0.0.1:7777` until
`04:46:18.866` — the monitor saw the *unit* go active ~200ms before the
listener existed and published into a closed port. Not introduced here:
the previous restart (0.1.41, 2026-07-30) logged **4** of them; this one
logged 1. Worth its own WI, not a rollback trigger.
