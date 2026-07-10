# Sprint 021 — corpus hygiene + miss log

**Status:** Active (started 2026-07-10 from korg proposal
[korg:337]; covers klams WIs #315–#318)
**Version:** workspace PATCH → `0.1.21`
**Derives from:** [../planning/roadmap.md](../planning/roadmap.md) queue
entry 021 · [../planning/2026-07-crossroads.md](../planning/2026-07-crossroads.md)
§5 (#1, #4) + §2.3 + §2.2 #3

## Goal

The first sprint of the post-crossroads retrieval-quality era. Stop the
corpus from re-polluting itself, start the file-type on-ramp so we stop
ingesting noise, and stand up the tuning-data feedback loop (miss log)
so 022's chunking work and 023's ranking work are measured against a
clean corpus and a real record of what agents wanted. Fast, and it
unblocks everything downstream: without it every eval measures a
polluted corpus and every chunking improvement compounds the leak.

## Scope (the four covered work items)

### #315 — Delete-before-reindex on edit (P0) · §5 #1

`scan_root` re-publishes a changed file's chunks but never deletes the
previous points ([crates/klams-scanner/src/lib.rs:73-89](../../crates/klams-scanner/src/lib.rs#L73-L89));
deletion only happens for *vanished* files (`lib.rs:98-113`). Changed
chunks create new points while old versions stay live and searchable —
the corpus has re-polluted itself on every edit since sprint 010's
one-time purge. The endpoint and store support already exist
(`POST /memory/knowledge/delete?source_file=`, `publish_delete` in
[publish.rs](../../crates/klams-scanner/src/publish.rs)) — the scanner
just never calls it on the changed-file path.

- **Fix:** in `scan_root`, before re-publishing a *changed* file that
  was previously indexed (a `cursor.get(&abs)` hit whose hash differs),
  call `publish_delete(base_url, bearer, &abs)` so stale points go
  before the new ones land.
- **Golden staleness assertion:** the e2e test's scenario-2 doc comment
  claims "the old content is gone" but only asserts the *new* nonce is
  present ([us3d_scanner_e2e.rs:5-6,89-92](../../crates/klams-service/tests/us3d_scanner_e2e.rs)).
  Add the missing assertion that the *old* nonce returns zero hits
  after the edit. This is the failing test that drives the fix.
- **One-time purge:** the fix stops *future* leaks; existing orphans
  from past edits remain. Since 022 runs a full re-index shortly after
  (and clearing the scanner cursor + rescanning now re-runs
  delete-before-reindex per file), the live purge is an **operational
  step** run on kubs0 post-deploy, documented in
  [docs/usage.md](../../docs/usage.md) — not new code paid for twice.

### #316 — Scanner file-type allowlist (P0) · §5 #4

The walker skips directories only ([walk.rs](../../crates/klams-scanner/src/walk.rs));
anything UTF-8 gets indexed — lockfiles, JSON fixtures, SVGs — a
meaningful slice of the ~94k corpus is noise. Add an extension
allowlist (source, docs, config prose worth retrieving) applied in
`walk()` so non-content files never reach the chunker. Overridable is
out of scope — a sane built-in allowlist is the YAGNI answer;
`.klamsignore` already exists for per-tree exclusions.

### #317 — Miss log + Grafana zero-hit panel · §2.3

Start collecting tuning data now. On the MCP `memory_search` path (the
real-traffic surface, [memory_search.rs:239-242](../../crates/klams-mcp/src/tools/memory_search.rs#L239-L242)),
record **zero-hit** and **low-top-score** results: query text, caller
(bearer agent identity), top score, hit count, kinds queried. Two
sinks: a Prometheus counter (`klams_search_misses_total{reason}`) for a
Grafana zero-hit-rate panel, and a durable Postgres `search_miss` row
(fire-and-forget, mirroring `touch_author_last_seen_at`) so "what did
agents want and not get" is queryable later. Misses drive 022 chunking
fixes, new scan sources, and the §2.1 lexical decision (024).

### #318 — Routing-rules rewrite of the agent-instructions blurb · §2.2 #3

`docs/klams-mcp-for-agents.md` *offers* klams; TMX proved offering is
worth ~nothing (0/15 vs 8/8 when enforced). Rewrite it as routing rules
("recall-shaped question → `memory_search` FIRST, before grep/web") and
propagate to the instruction files on cleo, kai, kubs0. Docs-only, zero
risk. (Propagation to the three machines is an operator step recorded
here; the repo change is the canonical blurb.)

## Acceptance

1. e2e scenario 2 asserts the old nonce is gone after an edit, and it
   passes against the docker test stack (delete-before-reindex works).
2. The scanner no longer indexes lockfiles/JSON-fixtures/SVGs; a unit
   test over a mixed-extension `walk()` fixture proves the allowlist.
3. A zero-hit `memory_search` increments `klams_search_misses_total` and
   writes a `search_miss` row with the query text + caller; the Grafana
   dashboard has a zero-hit-rate panel and passes the dashboard
   series-coverage contract.
4. The agent blurb is rewritten as enforced routing rules; docs updated.
5. `just gate` green (fmt, clippy -D warnings, tests). Coverage does not
   decrease.

## Outcome (2026-07-10 — implemented, gate green)

All four WIs landed; `just gate` green (fmt, clippy -D warnings, full
test suite). Both docker-gated acceptance tests were run live against
the test compose stack.

- **#315** — `scan_root` now calls `publish_delete` before re-publishing
  a changed, previously-indexed file
  ([lib.rs](../../crates/klams-scanner/src/lib.rs)); the double
  `cursor.get` was consolidated. The e2e golden staleness assertion
  passes *with* the fix and was proven to **fail without it**
  ("nonce_a still searchable after edit — stale chunk leaked"). The
  one-time purge is documented as an operator runbook in
  [usage.md](../../docs/usage.md).
- **#316** — `is_indexable()` allowlist in
  [walk.rs](../../crates/klams-scanner/src/walk.rs) (`ALLOW_EXT` /
  `ALLOW_NAMES` / `DENY_NAMES`); new hermetic test
  `walk_indexes_only_allowlisted_extensions`. yaml/yml kept (homelab
  compose/ansible/k8s); lockfiles denied by name.
- **#317** — migration `0010_search_miss.sql`; `SearchMiss` +
  `PostgresStore::insert_search_miss`; `klams_search_misses_total`
  counter + `incr_search_miss`; classification wired into MCP
  `memory_search` (caller threaded from the bearer identity), insert
  fire-and-forget. Grafana panel added + ansible-k handoff series table
  updated (contract test green). 5 unit tests for `classify_miss`/
  `kinds_label` + a live integration test
  `zero_hit_search_records_a_miss`.
- **#318** — routing-rules rewrite of
  [klams-mcp-for-agents.md](../../docs/klams-mcp-for-agents.md) blurb.
  Propagated to `~/.claude/CLAUDE.md` on **all three machines** —
  `kubs0`, `kai`, `cleo` (checksums verified identical, 2026-07-10).

**Observability home (ansible-k → k-homelab):** ansible-k is being
decommissioned, so its klams Grafana/Prometheus wiring (the
`klams-grafana.md` series table + the klams dashboard/scrape provisioning)
must move to k-homelab — filed as **korg k-homelab WI #362**. The
sprint-021 series (`klams_search_misses_total`) + panel were added to
`deploy/grafana/klams.json` and to the ansible-k handoff table (kept as
the transitional home so the contract test stays green until #362 lands).

**Deploy-time operator steps** (post-merge, on kubs0 — require the merged
code deployed, so they can't run before ship): install the new
scanner/service binaries; run the one-time stale-chunk purge
([usage.md](../../docs/usage.md)) or fold it into 022's re-index.

## Out of scope (deferred, tracked)

- Cross-file dedupe hazard (#324) and code-aware chunking (#320–#326) →
  sprint 022. The delete-before-reindex fix interacts with the global
  content-hash dedupe (#324): a chunk shared across two files is one
  point owned by the first — 022 scopes dedupe per source_file. For 021
  the corpus is overwhelmingly one-file-per-chunk, so delete-by-source_file
  is the correct, safe fix now; 022 hardens the shared-chunk edge.
- Ranking unification (#328) → sprint 023.
