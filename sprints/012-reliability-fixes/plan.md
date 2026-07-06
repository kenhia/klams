# Sprint 012 — Reliability fixes (activity feed + monitor)

**Status:** Active
**Branch:** `012-reliability-fixes`
**Date:** 2026-07-04
**Type:** Lightweight fix sprint (no full Spec Kit flow)
**Origin:** korg sprint proposal 175 ("Reliability bug-bash — klams + kdeskdash"),
**klams slice**. The kdeskdash slice shipped separately (its PR #10). korg items
#54, #55, #56, #31.

## Scope & decisions

Four deferred bugs. Two carried real approach decisions (made by Ken):

| # | Item | Decision |
|---|------|----------|
| #54 | Activity feed not globally newest-first | **Full time-merged cursor** (globally correct, not the interim Qdrant-only fix) |
| #55 | Monitor can't record klams-service's own `Down` | **Document the limitation** (no durable spool this sprint) |
| #56 | Service events carry `host=unknown` | gethostname (procfs) — clear fix |
| #31 | Memory detail routes 404 | **Already fixed on main** — verify only |

## #54 — global newest-first activity feed (the marquee change)

**Root cause:** `list_memories` was *sectioned* keyset pagination — all facts,
then all events, then all knowledge — so a new knowledge item sorted below every
fact/event, and the knowledge section itself came back **oldest-first** (Qdrant
`scroll` returns point-id / `now_v7` ascending order).

**Fix:** one unified `created_at DESC` keyset merge across all three kinds.
- `crates/klams-store/src/composite.rs` `list_memories_impl`: pages facts
  (Postgres), events (Postgres), and knowledge (Qdrant) with the **same**
  `(created_at, id)` keyset, then merges into one global newest-first page.
  New unified cursor `base64(ns:uuid)` (legacy `section:ns:uuid` still decodes).
  The merge is a pure, unit-tested fn `take_merged_page` (`merge_tests`).
- `crates/klams-store/src/qdrant.rs`: knowledge now scrolls `order_by(created_at
  DESC, start_from=…)` instead of point-id order. Requires a **datetime payload
  index** on `created_at`, created idempotently in `connect()` (Qdrant builds it
  over existing points — no manual reindex loop). Window + exact keyset are still
  enforced client-side on the RFC3339 payload.

**Acceptance:** feed returns rows in global `created_at DESC` across all kinds;
pagination preserves ordering across boundaries; viewport renders newest-first
with no client sort. Regression covered by `merge_tests` (interleaving, ties,
drain, saturation) + the `#[ignore]` cross-store integration test.

## #56 — real host on Service events

`crates/klams-monitor/src/main.rs` `default_host()` read `$HOSTNAME`, which
systemd does not export → always `unknown`. Now reads `/proc/sys/kernel/hostname`
(equivalent to `gethostname(2)`, dependency-free, offline-safe), falling back to
`$HOSTNAME` (now also set via `Environment=HOSTNAME=%H` in the unit) then
`unknown`.

## #55 — documented self-sink limitation

`klams-monitor` posts to `klams-service`, so it can't record `klams-service`'s
own `Down` (publish fails, no buffer, state advances). Documented in the module
header + at the publish-failure site + the deploy unit. The outage stays
**reconstructable from the gap** between the last good `Up` and the recovery
`Up`. (A durable spool was explicitly deferred.)

## #31 — already fixed on main (verified)

The in-place details pane (`viewport/src/lib/components/MemoryDetails.svelte`
wrapped in `<details>` on the Activity + Authors views) already replaced the dead
`/facts|events|knowledge/[id]` navigation. Verified: `pnpm check` 0/0,
`pnpm test` 39 passed. No code change. (Real `[id]` permalink routes remain an
out-of-scope follow-up.)

## Validation & ship gate

- `just gate` (fmt + clippy `-D warnings` + `cargo test --workspace`) green;
  new `merge_tests` (5) pass. Viewport `pnpm check`/`test` green.
- **Live-Qdrant verify pending (kubs0)** for #54: confirm the `created_at`
  datetime index builds over the ~94K-point collection and the Activity feed
  renders globally newest-first (facts/events/knowledge interleaved by time)
  across "Load more". Also worth a spot check that #56 stamps the real host on a
  driven Service event. These gate the `resolved → closed` transition.
