# Sprint 060 — `--restore-from` into a fresh target, and the tool-version trap

Proposal korg:5074, a slice of the low-hanging-fruit program korg:5082
(2026-11-26), overseen from a session on cleo. Run as karc leg
`klams-68e046` on kubs0. Covers **WI 4588**. Version `0.1.60`.

## Goal

Finish WI 4588, found by the November 2026 restore drill. Its point 1 (PG 18
client tools against the PG 16 server) was closed by Ken's `pg_bin_dir` pin on
2026-11-12. Two points remained:

- **Point 3:** `--restore-from` without `--force` failed on a never-migrated
  database. The guard probed `facts`/`events` and died with
  `relation "facts" does not exist`. That target is exactly the DR case.
- **Point 4:** the pin is a hand edit in `/etc/klams/klams.toml`, so the
  example config and setup doc should say when to set `pg_bin_dir`.

The overseer added two requirements. The client/server major-version check the
WI recommends goes in as a **warning, not a refusal**. The sprint 006
round-trip test must be extended so that it would have caught both defects.

## Acceptance

`klams-service --restore-from <date>` with no `--force` restores a nightly
pair into a fresh, never-migrated stack on kubs0.

## Premise check (sprint start)

- **WI 4588 point 3 — premise holds.** `probe_postgres_non_empty` ran
  `SELECT (SELECT COUNT(*) FROM facts) + (SELECT COUNT(*) FROM events)`
  unconditionally. The new test reproduced the drill's exact error against
  the old code (see below).
- **The Qdrant probe — premise drifted.** It already read a missing
  collection as empty. But it read *every* non-success as empty, including
  auth failures and 5xx responses. Such an answer is not evidence of
  emptiness, and it would let the guard wave a restore through over live
  data. Tightened: only a 404 is empty, and anything else refuses.
- **WI 4588 point 4 — premise drifted.** `deploy/config/klams.example.toml`
  already documented `pg_bin_dir` (sprint 035, #776), but softly ("when the
  pg_dump on PATH doesn't match"). `docs/setup.md`'s restore section did not
  mention it at all. Both were strengthened rather than written new.

## What shipped

- **`restore.rs`.** The Postgres probe checks `to_regclass` per table, and
  a missing table counts as zero rows. The Qdrant probe treats only a 404 as
  empty.
- **`klams-store::backup::postgres::check_tool_version`.** Runs the
  `pg_dump --version` that backups would actually run (same `pg_bin_dir` /
  `KLAMS_PG_BIN_DIR` resolution) against `SHOW server_version_num`, with a
  5s bound. It returns a warning naming the pin to set. The parsers and the
  comparison are unit-tested.
- **Warning at backup time.** `run_once_inner` logs it at `warn` before the
  dump. If the check itself cannot run, that goes to `debug`, because a
  missing `pg_dump` makes the dump fail loudly anyway.
- **Warning in `--validate-backup-config`** (enabled backups only). This is
  now the one backend contact that check makes. A mismatch prints
  `warning:` and an unreachable server prints `note:`. Neither changes the
  exit code. The function became `async`.
- **The round-trip test** (`tests/restore_roundtrip.rs`) gained
  `restore_into_never_migrated_target_without_force`. It asserts
  `check_tool_version` is clean for the tools the backup uses, backs up a
  seeded fixture, creates a brand-new database, deletes the collection, and
  restores with `force=false`. It then compares fact/event counts, a
  10-row sample, and the Qdrant point count, and drops the database.
- **Docs.** The example config, `docs/setup.md` (restore section: the pin,
  and that a fresh target needs no `--force`), and `docs/usage.md` (the
  `[backup]` block, the validate behaviour, the recipe table) are all
  updated.

## The test catches both defects (TDD evidence)

- Against `HEAD`'s `restore.rs`, the new test fails with the drill's own
  message: `postgres probe: error returned from database: relation "facts"
  does not exist`.
- With `KLAMS_PG_BIN_DIR=/usr/lib/postgresql/18/bin` (kubs0's PATH default),
  it fails at the version assertion: `pg_dump is PostgreSQL 18 but the server
  is 16 …`. With the 16 tools it passes.

## The drill (acceptance, run 2026-11-26 ~18:17 UTC on kubs0)

- Scratch compose project `klams-drill060`: postgres:16 and qdrant v1.18.0
  on 61410–61412, fresh volumes. The target had 0 public tables and 0
  collections.
- Pair `postgres-2026-11-26.dump` / `qdrant-2026-11-26.snapshot` was copied
  from `/backup/klams`. The dump header reads `Dump Version 1.15-0`, dumped
  by pg_dump 16.16, so the pin is holding.
- This branch's `klams-service --restore-from 2026-11-26` with **no
  `--force`** returned `OK restore … elapsed_ms=4287`.
- Restored facts 73 / events 50 / authors 50 / migrations 13 and 234,304
  points, green. Live read the same at the same moment: 73/50/50/13 and
  `knowledge_items_v2` 234,304.
- `--validate-backup-config` with the pin printed OK. With the pin
  removed it printed the 18-vs-16 warning, and the exit code was still 0.
- Teardown: `docker compose down -v` (volumes removed) and the copied pair
  deleted. `/etc/klams/klams.toml` was not touched.

## Gates

`just gate` green. `just test-integration` green (149 passed, 0 failed).
`backup_validate_config_cli` (ignored suite) is 4/4.

## Decisions

- **Warning, not refusal** (overseer's call, Ken can reverse). Ken's recorded
  decision covered only the pin, and a refusal would change behaviour for
  anyone running backups.
- **`--validate-backup-config` now contacts Postgres.** Its doc said "no
  backends are contacted". The overseer asked for the warning there, and the
  server's version cannot be known without asking, so the contract is
  narrowed rather than broken: the contact is advisory, bounded, and only
  made when backups are enabled.
- **The validate CLI test's config now points at `127.0.0.1:1`** instead of
  `:5432`. On kubs0, 5432 is the live database, and the new version probe
  would otherwise make a failed-auth attempt against production on every
  test run.

## Not done here, deliberately

- **The provision script still does not set `pg_bin_dir` by default.** No
  k-homelab backup row covers `/etc/klams/klams.toml` as a file. On a rebuild,
  k-homelab's `dr/kubs0.md` step 10 sources it from this repo (the
  provision script renders it from `klams.example.toml`). That makes the
  example config the right place for the guidance, and it is where the
  guidance now sits. A Debian path in the defaults would break the
  operator-surface rule in `AGENTS.md` ("generic or fail loudly"). The
  new warning is the loud half: on a rebuilt host that forgot the pin, the
  first `--validate-backup-config` or nightly run says so by name.
