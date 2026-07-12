# Sprint 023 — multi-host scanning + host identity

**Status:** Active (started 2026-07-11 from korg proposal [korg:413];
covers klams WIs #407–#411)
**Version:** workspace PATCH → `0.1.23`
**Derives from:** operator need — repos are now split across kubs0 and
kai (cleo later), so kai-repo knowledge is entirely absent from klams.
Inserted ahead of ranking unification (now sprint 024).

## Goal

Give every scanned chunk a **host identity** and start scanning **kai**,
so the corpus covers the whole fleet's source instead of just kubs0's.
Today the scanner sends `machine: None` and two identity operations key
on the file path alone — so a second scanner can't be pointed at klams
without the two hosts corrupting each other. This sprint closes that
correctness gate, makes retrieval return a fully-qualified `(host, file)`
that agents can act on directly, and lights up kai.

Direction (decided 2026-07-11): **per-host scanner** running on the
actual host (not central mount-scan). For kai — Linux, same binary —
this means local reads (fast), a self-describing host (`gethostname`),
and a resilient failure mode: a host being down means its data goes
*stale*, never deleted. Central NFS mount-scan (for hosts that can't run
the scanner, e.g. Windows/cleo) is captured as a future option
([klams #406](../../)) with the `NOT_MOUNTED` sentinel guard against the
mount-flap-causes-mass-prune failure.

## Scope

### #407 — Scanner records its host (populate `machine`)

The scanner sends `machine: None`
([publish.rs](../../crates/klams-scanner/src/publish.rs)). Resolve the
host (`gethostname`, or a config key — the monitor's host=unknown bug
WI #56 is the cautionary tale: systemd doesn't export `$HOSTNAME`) and
send it as `machine` on every `IndexKnowledgeRequest`. Keep resolution
in a single host-source function so the future mount-scan mode (#406)
can swap in a derive-from-scan-root-path variant. The field already
exists end-to-end; this fills it.

### #408 — Host-aware chunk identity (correctness gate)

Both `delete_by_source_file`
([qdrant.rs:406](../../crates/klams-store/src/qdrant.rs)) and the
content-hash dedupe probe (sprint 022 #324) key on `file` alone. Two
hosts sharing a path (`/home/ken/src/…` on both) would cross-delete and
collapse. Make delete scope to `(machine, file)` and the dedupe probe
key on `(machine, file, content_hash)` — threading the host through the
`POST /memory/knowledge/delete` endpoint + the scanner's
`publish_delete`, and through `find_knowledge_by_content_hash`.
Regression test: two hosts, same path, distinct content → two points;
deleting one host's file leaves the other intact. **This must land
before kai's scanner is enabled.**

### #409 — Surface host in the knowledge projection

`PublicMemoryContent::Knowledge`
([memory.rs](../../crates/klams-types/src/memory.rs)) exposes
text/source_path/repo but not the host. Add it (read `machine` back from
the payload) so `memory_search` returns `(host, file)` — an agent knows
a hit lives on kai vs kubs0 with no extra klams-fact or korg lookup.

### #410 — Deploy klams-scanner on kai

Install the release binary on kai (Linux, same build), add
`/etc/klams/scanner.toml` (roots incl. `/home/ken/src`, a dedicated
`kai-scanner` write token hot-added to kubs0's `klams.toml`, `url =
http://kubs0:7777`), and the systemd oneshot + timer mirroring kubs0.
Verify a kai file is searchable with `machine=kai`.

### #411 — Rescan both hosts (deploy-time)

Backfill the host everywhere: on kubs0 invalidate the cursor (`UPDATE
file_cursor SET mtime_ns=0, content_hash='reindex'` via `python3` — no
`sqlite3` CLI on kubs0) and rescan so existing chunks gain
`machine=kubs0`; run kai's scanner for `machine=kai`. kubs0 was
re-indexed to scanner-v2 in 022 but with `machine=None`, so it needs
this host backfill.

## Ride-along docs fix

Correct the 021/022 re-index runbook in
[usage.md](../../docs/usage.md): it calls `sqlite3` (not installed on
kubs0) — use `python3`. (Folding the deferred fix in here per Ken.)

## Acceptance

1. Scanner chunks carry `machine`; delete + dedupe are host-scoped;
   regression test proves same-path-different-host stays two points.
2. `memory_search` knowledge results include the host.
3. kai's scanner is live; a kai-only file is retrievable with
   `machine=kai`; kubs0 files show `machine=kubs0`.
4. `just gate` green; main CI green.

## Out of scope (deferred, tracked)

- Central NFS mount-scan mode + `NOT_MOUNTED` guard → [klams #406].
- Ranking unification → sprint 024 (renumbered from 023).
- cleo/Windows source → via #406 when it lands, or a mirror-to-Linux.
