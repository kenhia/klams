# Sprint 059 — `kdash:klams:status` publisher, and kubs0's idle scanner

Proposal korg:4617, slice 4 of program korg:4619 (kpidash row 2: Claude
limits card and korg + klams card), overseen from a session on cleo. Run
as karc leg `klams-ebb307` on kubs0. Covers **WI 4612**. Version `0.1.59`.

## Goal

1. Publish klams's status to the dashboard Redis as `kdash:klams:status`,
   to the contract kdashdata shipped in korg:4615: schema
   `contracts/schemas/kdash-klams-status.schema.json`, v1, pinned at
   kdashdata `d57e1dc` (overseer comment 4658 on the proposal).
2. Settle why kubs0's scanner author `klams-scanner` has written nothing
   since 2026-11-10 while its timer fires hourly, because that decides
   what the card shows for a scanner that ran without writing.

## Acceptance (from WI 4612)

- Gate green, with a test validating a built record against the
  kdashdata schema.
- The live key holds a schema-valid record matching `/api/overview` and
  `/healthz` read at the same time.
- The wrap-up states the scanner finding and what the widget should show
  for a scanner that ran without writing.

## Premise check (sprint start)

- **WI 4612 — premise holds.** `klams-scanner`'s `last_seen_at` read
  `2026-11-10T06:09:06Z` on `/api/overview`; `klams-scanner.timer` was
  firing hourly (last at 00:10Z, next 01:10Z). Both halves of the
  observation reproduced.

## The scanner finding — it runs, and has had nothing to write

**Reading 1 of the two the WI offered: healthy and idle.** Not a bug.

The last run that wrote anything, and the first one after it:

```text
Nov 09 22:09:05 kubs0 klams-scanner[212456]: 2026-11-10T06:09:05.805611Z  INFO klams_scanner: klams-scanner starting roots=1 interval_secs=3600 once=true host=kubs0 max_input_tokens=32768
Nov 09 22:09:05 kubs0 klams-scanner[212456]: 2026-11-10T06:09:05.978690Z  INFO klams_scanner: cleared stale chunks before reindex path=/home/ken/src/ai-agents/agent-skills/sprints/4483-cut-status-drawdown-lessons.md deleted=9
Nov 09 22:09:06 kubs0 systemd[1]: klams-scanner.service: Deactivated successfully.
Nov 09 23:09:05 kubs0 klams-scanner[258277]: 2026-11-10T07:09:05.842038Z  INFO klams_scanner: klams-scanner starting roots=1 interval_secs=3600 once=true host=kubs0 max_input_tokens=32768
Nov 09 23:09:05 kubs0 systemd[1]: klams-scanner.service: Deactivated successfully.
```

and every run since looks like that second one, through the one this
sprint watched fire:

```text
2026-11-13T17:10:56-08:00 kubs0 klams-scanner[4081550]: 2026-11-14T01:10:56.993319Z  INFO klams_scanner: klams-scanner starting roots=1 interval_secs=3600 once=true host=kubs0 max_input_tokens=32768
2026-11-13T17:10:57-08:00 kubs0 systemd[1]: klams-scanner.service: Deactivated successfully.
```

`systemctl show klams-scanner.service`: `Result=success`,
`ExecMainStatus=0`, `ExecMainExitTimestamp=@1794618657` (01:10:57Z).

Why that means idle rather than broken:

- **Nothing under the root changed.** The scanner's one root is
  `/home/ken/src` on kubs0 (`/etc/klams/scanner.toml`). The newest commit
  in any repo there is agent-skills at 2026-11-09 21:48 PST (05:48Z on
  11-10), which the 06:09Z run indexed — the line above. A `find` for
  files modified since 2026-11-09 22:00 PST, outside `.git`, `target`
  and scratch, returns two: `klams/.env` (gitignored, so the walker skips
  it) and this sprint's own `.korg-sprint-proposal` marker. Ken's active
  checkouts are on kai, whose scanner did write (`kai-scanner`
  `last_seen_at` 2026-11-13T21:19Z).
- **The walker saw the tree.** A walk that returned no files would have
  marked every cursor entry vanished and pruned it, logging `pruned` for
  each — there are none. No `scan failed` warning either, and
  `validate_roots` refuses an unreadable root at start. So every file was
  walked and short-circuited on `mtime_unchanged`.
- **The cursor agrees.** `/var/lib/klams/scanner.sqlite` was last
  modified at 2026-11-09 22:09 PST — the 06:09Z run.
- **`last_seen_at` is a write stamp.** klams bumps it on writes only:
  `kmon` made 437 searches since the service started on 11-13 and its
  `last_seen_at` still reads 11-10. A scanner that runs and reads nothing
  makes no request at all.

### What the card should show for a scanner that ran without writing

**Healthy — green, not red.** The record now carries both stamps. The
rule the card should apply, per scanner:

- `last_ran_at` within `expected_interval_s` (plus grace) → **healthy**,
  whatever `last_wrote_at` says. Draw the freshness bar from
  `last_ran_at`, and show `last_wrote_at` as secondary text ("wrote 3d
  ago"), never as the alarm.
- `last_ran_at` older than that → **stalled** (red): the timer or the
  unit has stopped.
- `last_ran_at` null → **run time unknown** (neutral/grey, not red): the
  publisher has no source for it. Fall back to `last_wrote_at` for the
  bar, but label it as a write age. Today this is kai's scanner (below).

kubs0 today reads `last_wrote_at: 1794290946` (11-10 06:09Z) with
`last_ran_at: 1794618657` (minutes old): green, "wrote 3d ago".

## Decisions

**D1 — the publisher is `klams-monitor`, not klams-view.** WI 826 and
WI 4612 leaned to klams-view and left the leg free to choose with a
reason. The monitor won on three counts: it already runs on kubs0 as an
*external* observer with `REDISCLI_AUTH` wired, so it can publish `down`
when klams-service is gone — exactly what the contract requires, and
something a viewer that proxies klams would do only by accident; it lives
in the repo this leg holds, so no second repo or deploy path; and klams
`/v1/authors` plus `/healthz` plus `/metrics` are the same three sources
klams-view's `/api/overview` reads, so nothing is lost. Counts are summed
exactly as `/api/overview` sums them (`facts` = Σ `counts.writes`) so the
card and the viewer cannot disagree.

**D2 — write through `kdash-pub set`, not the `redis` crate or the
`kdash_pub` Rust crate.** The contract's publisher route is `kdash-pub`
(comment 4658), and the CLI does khlenv endpoint resolution (CD-4), auth
from `REDISCLI_AUTH` or `/etc/khomelab/secrets.env`, `ts` handling and
the key-grammar check. The Rust crate would do the same in-process but
is a git dependency on the private khlenv repo, which klams's CI cannot
fetch. One exec a minute costs nothing. The legacy `[kpidash]` card keeps
its direct `redis` connection; unchanged.

**D3 — `last_ran_at` from systemd, local scanners only.** The publisher
reads `systemctl show <unit> -p Result -p ExecMainExitTimestamp` for a
scanner with a `unit` configured, and reports the exit time only when the
last run's `Result` is `success`. A failed last run is not one the card
should call healthy, so it reports null (and `last_wrote_at` ages on).
kai's scanner has no source on kubs0 → `null`, which the contract defines
as "no source". Giving kai's run time a source is a decision, filed (see
Follow-ups).

**D4 — when `/v1/authors` fails but `/healthz` answers**, counts are null
and `scanners` is `[]`. An entry with `last_wrote_at: null` would claim
"never wrote", which is false. `health` stays klams's own verdict.

**D5 — `new_per_day` is `[]`.** klams has no daily series to copy, and the
contract forbids re-deriving (and allows `[]`). klams-view's activity
histogram walks `/v1/memories` per request; doing that every minute from
the monitor would be re-deriving, and expensive. Only the standalone card
(not built) draws it.

**D6 — the schema test vendors the schema.** `tests/fixtures/
kdash-klams-status.schema.json` is a byte copy of kdashdata at `d57e1dc`
(md5 `9e374cb9933f68f278105936621dbdb4`; kdashdata `main` had moved to
`d995faa` by the time of the copy, with this file unchanged). A contract
change is a new key, so the copy never needs a silent refresh. The test
also proves the validator by accepting the schema's own examples and
refusing every counterexample.

## What shipped

- `crates/klams-monitor/src/kdash_status.rs` — config, pure
  `build_record`, systemd and Prometheus parsers, and `StatusPublisher`
  (observe → build → `kdash-pub set kdash:klams:status`).
- `klams-monitor` — `[kdash_status]` section (inert when absent); a
  60-second publisher task beside the unit poller; `--kdash-status-once`
  (publish one record and exit, polling no units and posting no events)
  and `--dry-run` (print it instead).
- `crates/klams-monitor/tests/kdash_status_schema.rs` — healthy,
  degraded, down, author-read-failure and missing-author records all
  validate; the ran-without-writing case carries both stamps.
- Docs: `docs/usage.md` (the record, the config, the one-shot, the
  scanner idle note), `docs/architecture.md` §2.4,
  `deploy/config/monitor.example.toml`.

## First publish — fired by trigger, not timer

Before ship, from the branch build, as the `klams` user, through the
installed `kdash-pub` (endpoint `rpi53:6379`, auth from
`/etc/khomelab/secrets.env`):

```text
sudo -u klams ./klams-monitor --config .scratch-agent/059/monitor.toml --kdash-status-once   → rc=0
kdash-pub get kdash:klams:status                                                            → rc=0
python3 jsonschema Draft202012 against the d57e1dc schema                                    → schema-valid
```

Live key vs `/api/overview` read in the same minute: knowledge 228956 =
228956, facts 66 = 66, events 50 = 50, version 0.1.58 = 0.1.58; kai
`last_wrote_at` 1794604757 = `2026-11-13T21:19:17Z`, kubs0
`last_wrote_at` 1794290946 = `2026-11-10T06:09:06Z`, kubs0
`last_ran_at` 1794618657 = the 01:10:57Z run above. The `down` path was
proved at runtime too: a dry run pointed at a dead port printed a
`health: "down"` record with every klams value null.

## Repaired in passing

- **The scanner's journal could not tell an idle run from a broken one.**
  An idle run logged only "starting", which is how four days of healthy
  runs read as a stopped scanner. `scan_root` now ends with one `scan
  complete root=… walked=N indexed=N pruned=N` line per root. Logging
  only; `just gate` and the integration suite prove it. (Took
  `clippy::too_many_lines` on `scan_root`, alongside the
  `too_many_arguments` it already allowed — the repo's precedent for its
  other long pipeline functions.)

## Follow-ups

- Filed **WI 4639**: a source for a remote scanner's `last_ran_at` (kai). Needs a
  decision (klams heartbeat API vs a new kdash key vs leave null) — see the WI.

## For k-homelab, to deploy

Nothing in k-homelab is required. `kdash-pub` is already on kubs0,
`/etc/khomelab/secrets.env` already feeds `klams-monitor` through its
`10-khomelab-secrets.conf` drop-in, and the `klams` account is already a
`secrets_group_members` entry in `manifests/kubs0.yml`. The deploy is
klams's own: `deploy-kubs0` installs the binaries, and
`/etc/klams/monitor.toml` (klams-owned) gains the `[kdash_status]`
section below before the monitor restarts.

```toml
[kdash_status]
interval_secs = 60
kdash_pub = "/usr/local/bin/kdash-pub"

[[kdash_status.scanners]]
name = "kai"
author = "kai-scanner"

[[kdash_status.scanners]]
name = "kubs0"
author = "klams-scanner"
unit = "klams-scanner.service"
```
