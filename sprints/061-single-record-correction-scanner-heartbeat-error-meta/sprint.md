# Sprint 061 — single-record correction, scanner run heartbeat, error code in `_meta`

Proposal korg:5107, slice 3 of the low-hanging-fruit program korg:5112
(2026-11-27), overseen. Run as karc leg `klams-badd2f` on kubs0. Covers
**WI 4835** and **WI 4639**, plus the error-format ruling from korg:4767
item 10. Version `0.1.61`.

## Goal

Three klams contract changes Ken ruled on at the 2026-11-27 staff meeting:

1. **WI 4835, option 1.** `supersede_propose` accepts zero `retire_ids` when
   `text` is given — "replace the keeper with this text" — so a non-manage
   identity has a klams path to correct another author's stale record.
   Additive: merge calls behave exactly as before; zero retirees with no text
   is still refused.
2. **WI 4639, option 1.** Each scanner tells klams it ran (one authenticated
   heartbeat per run); klams exposes it per author as `last_ran_at` on
   `/v1/authors`; the kubs0 publisher copies it into `kdash:klams:status` for
   every scanner host. `last_seen_at` stays a write stamp.
3. **Error format, "fix it, keep the text".** A failed MCP call puts
   `error_code` in the `CallToolResult`'s own `_meta`; the JSON envelope in the
   text content stays byte-for-byte as it was, so klams-mind's nested parse
   keeps working.

## Acceptance (from the proposal; all same-day)

- 4835: a non-manage identity files a text-only proposal against another
  author's record and a manage identity accepts it; a merge still works; tests
  cover both. Live check with throwaway records, cleaned up.
- 4639: after one **triggered** run of kai's scanner, `kdash:klams:status`
  shows a non-null `last_ran_at` for kai.
- Errors: a deliberately failing call shows the code in `_meta` and the text
  envelope unchanged.

## Premise check (sprint start)

- **WI 4835 — premise holds.** `supersede_propose::run` refused an empty
  retire set (`retire_ids must name 1..=20 distinct records`), and migration
  0013 carried `CHECK (cardinality(retire_ids) >= 1)` — live constraint name
  `supersede_proposals_retire_ids_check`, read from prod `pg_constraint`.
  The accept path already handled an empty retire set with `text` correctly
  (writes the replacement, retires the keeper), so the change is validation +
  constraint only.
- **WI 4639 — premise holds.** `kdash_status.rs` took `last_ran_at` from
  `systemctl show` for a scanner with a `unit`, so only kubs0 had one; kai's
  entry in the live `/etc/klams/monitor.toml` has no unit.
- **Error format — premise holds.** `envelope_result` set `is_error`, the text
  content and `structured_content`, and left `CallToolResult.meta` `None` —
  exactly the quirk klams memory `01a1c36c-…` describes.

## Decisions

- **Correction proposals reuse the merge machinery unchanged.** No new tool,
  no new status. Migration 0014 swaps the CHECK for
  `cardinality(retire_ids) >= 1 OR text IS NOT NULL`, so the database refuses
  the meaningless shape too, not only the handler. `retire_ids` got
  `#[serde(default)]`, so a correction may omit the field entirely. The
  replacement is authored by the proposer — the same rule a merge already has.
  The tool description and the server instructions now name the shape ("no
  `retire_ids`, just the corrected `text`"); the server-instructions block is a
  call site whenever a tool contract changes.
- **Heartbeat shape: `POST /v1/authors/me/ran` with `{"ok": bool}`.**
  - Under `/v1/authors` because what it writes is a field of the author
    projection; `me` because it can only ever write the caller's own row
    (the bound identity from `X-Homelab-Agent` — no author id is accepted).
  - **`Write` scope**: every scanner already holds it, so nothing new to
    grant (brief: "no new scope").
  - Idempotent: `UPDATE authors SET last_ran_at = now(), last_run_ok = $ok`.
    A repeat only moves the time forward. It never touches `last_seen_at`.
  - Columns on `authors` (migration 0015), not a side table: the projection
    already reads that row, and both columns are nullable, so every existing
    author reads `null` until its first heartbeat.
  - `ok` is `false` when any root's scan failed. That carries information
    systemd could not: the scanner logs a failed root as a warning and still
    exits 0, so `Result=success` was reported for runs that scanned nothing.
  - Best-effort: a failed heartbeat is a `warn!` and never fails the run.
- **One source for every scanner: the heartbeat.** The brief asked to pick one
  per scanner. kubs0's scanner moved off systemd too, so the publisher has one
  path; `ScannerConfig.unit`, `parse_last_ran` and the `systemctl` call are
  gone. A leftover `unit = ` in a deployed config is silently ignored (the
  config struct does not deny unknown fields — checked), so the live
  `/etc/klams/monitor.toml` needs no edit to keep working.
- **`last_ran_at` is published only for a run that reported success** — the
  same semantics the systemd source had (sprint 059: "a failed last run is not
  a run the card should call healthy"). A failing scanner therefore reads as
  `null`/stale on the card, as it did before.
- **No kdashdata change.** The schema's `last_ran_at` ("Unix seconds the
  scanner last RAN … null when the publisher has no source") is unchanged in
  shape and meaning; kai's value simply stops being null. Read at kdashdata
  `contracts/schemas/kdash-klams-status.schema.json` (kai, version
  `b346f613c5bf817c`). No relay text needed.
- **kai's scanner is delivered by this repo**, not k-homelab: the
  `deploy-kubs0` skill's "The same release reaches kai" step runs
  `just deploy-remote kai klams-scanner`. So the deploy is the ship turn's own
  work, not a relay.
- **Error `_meta`**: `envelope_result` now also serializes the envelope's
  `ErrorMeta` (`error_code`, plus `retry_after_seconds` / `window_max_days`
  when set) into `CallToolResult.meta`. Text content and `structured_content`
  are unchanged; the unit test asserts all three.

## What shipped

- `crates/klams-mcp/src/tools/supersede_propose.rs` — zero retirees allowed
  with `text`; shape checks moved into `validate_shape` (clippy's 100-line
  limit).
- `migrations/0014_supersede_correction.sql`,
  `migrations/0015_author_last_ran.sql`.
- `klams-store`: `record_author_run`; `last_ran_at` / `last_run_ok` on
  `AuthorWithCounts{,Out}` and in both author SELECTs.
- `klams-api`: `POST /v1/authors/me/ran` (`Write`); the two fields on the
  author projection. `klams-types::PublicAuthor` gained both with
  `#[serde(default)]`, so a newer client still decodes an older server.
- `klams-client::report_run`; `klams-scanner` sends it after every run.
- `klams-monitor`: `kdash:klams:status` reads `last_ran_at` from the author
  list for every scanner; the systemd path is removed.
- `crates/klams-mcp/src/tools/mod.rs` — `envelope_result` fills
  `CallToolResult.meta`.
- Docs: `docs/klams-mcp-for-agents.md` (correction shape, where the error code
  is), `docs/usage.md` (heartbeat section, kdash source, author review),
  `docs/architecture.md` (author projection, scope table, supersede
  proposals), `deploy/config/monitor.example.toml`.

## Tests

- Hermetic: `a_text_only_correction_replaces_the_keeper_on_accept`;
  the existing "refuses shapes" case now reads "empty retire set and no text";
  `error_result_puts_code_in_own_meta_and_keeps_the_text_envelope`;
  `write_token_may_report_a_run`, and the heartbeat added to the read-only
  refusal list; monitor `a_remote_scanner_run_time_comes_from_its_heartbeat`
  plus the existing schema tests moved onto the heartbeat source.
- Docker stack (`just test-integration`, all green):
  `a_text_only_correction_is_accepted_end_to_end` (exercises migration 0014
  against real Postgres, including the no-text refusal),
  `a_failed_call_carries_its_code_in_the_result_meta` (on the wire, over the
  MCP HTTP transport), `run_heartbeat_sets_last_ran_at_and_last_run_ok`, and
  the unchanged merge test `propose_then_accept_retires_into_the_merge`.
  The heartbeat test cannot assert "null before" or "`last_seen_at`
  untouched": the bound test author is shared and long-lived, and parallel
  tests write through it. The SQL names only the two run columns.
- `just gate` green.

## Repaired in passing

- None outside scope. (A self-inflicted regression caught by the build: the
  first `_meta` edit dropped the `structured_content` assignment; restored, and
  the unit test now asserts `structured_content` is unchanged so it cannot
  recur silently.)

## Follow-ups

- After deploy: supersede klams memory `01a1c36c-162a-7202-bda8-2ee72cd40f35`
  (its second paragraph describes the now-fixed `_meta` quirk) — ship-turn
  work, per the brief.
- The live `/etc/klams/monitor.toml` still carries `unit =
  "klams-scanner.service"` on the kubs0 entry. Harmless (ignored); remove it
  whenever that file is next touched by whatever renders it.
