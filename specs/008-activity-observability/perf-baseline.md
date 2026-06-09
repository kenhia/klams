# Perf baseline — sprint 008

> Generated 2026-06-01T00:37:25.028268964Z by `just bench-run` on `kubs0`.
> Fixture seed: `0x0000c0ffee000008` · Store: 10,000 facts, 50,000 knowledge items.

## `memory_search` latency (100 samples across 10 queries)

| Metric    |              Value |
| --------- | -----------------: |
| p50       |       56.4 ms |
| p95       |       146.9 ms |
| p99       |       210.8 ms |
| min / max | 39.8 ms / 215.3 ms |
| mean      |      77.8 ms |

## Sample queries

- `"widget deploy outcome"`
- `"kubs0 backup window"`
- `"postgres tuning runbook"`
- `"qdrant collection schema"`
- `"incident timeline"`
- `"restart policy"`
- `"agent confidence calibration"`
- `"fact dissent resolution"`
- `"embedding pipeline latency"`
- `"dashboard scrape target"`

## Notes

- Run against a quiescent store; concurrent writes during the run will skew the numbers.
- SC-006 threshold (`memory_search` p95 < 1 s) is not enforced by this harness; this file surfaces the measurement. Tuning is gated on user review.

## Sprint 009 acceptance (T046)

Recorded 2026-05-31 on `kubs0` after walking
[specs/009-stability-attribution/quickstart.md](../009-stability-attribution/quickstart.md)
end-to-end.

### SC-001 — loopback CLOSE_WAIT regression

PASS. 18 h soak (`just soak --duration 18h`) completed 2026-05-31:
259 201 / 259 201 requests, 0 failures, max `CLOSE_WAIT` = 0 across
all 2 161 samples. Full report:
[specs/009-stability-attribution/soak-report.md](../009-stability-attribution/soak-report.md).

### SC-003 — post-cutover REST writes attributed away from `system`

PASS. Cutover went live 2026-05-28; the 26 legacy `system`-stamped
facts in the live store all predate the cutover (2026-05-20 / 2026-05-21).
Only one fact has been written through the REST surface since:
the `alice-quickstart` UserFact from step 2 of the quickstart,
correctly attributed to author `alice`. Post-cutover share of
`system`-attributed REST writes: **0 / 1 = 0%** (well under the 5%
threshold, though on a much smaller sample than the 200-row design
target — live REST traffic since the cutover is genuinely that low).

Stronger evidence comes from the T015 contract tests in
[`.scratch/user-tests.md`](../../.scratch/user-tests.md), which
exercise the wiring directly: T1 (bound `alice` token → `alice`
author), T2 (unbound token → `system` fallback), T3 (uppercase
`agent_name` rejected at startup with `Charset` error before the
listener binds), and T4 (two tokens sharing the same `agent_name`
resolve to the same `author_id`). All four passed.

### SC-004 — `reattribute-system` per-author counts, idempotency

PASS on **idempotency / lost-author invariant**; no recovery
exercised because the legacy `system`-stamped rows in this
deployment have no `register_author` antecedents in `events`
(the events table only contains 3 rows total, all post-cutover).

| Run                  | facts.total | reassigned_recovered | reassigned_lost | left_as_system |
|----------------------|------------:|---------------------:|----------------:|---------------:|
| dry-run (before)     | 26          | 0                    | 0               | 26             |
| **apply**            | 26          | 0                    | 0               | 26             |
| dry-run (after)      | 26          | 0                    | 0               | 26             |

Same shape for `events` (1 row, all three runs); `knowledge_items`
empty. Reports archived at `/tmp/reattribute-dryrun.json`,
`/tmp/reattribute-apply.json`, `/tmp/reattribute-dryrun2.json`. The
tool's classifier correctly identified that no provenance was
recoverable and chose `left_as_system` over forcing rows onto
`lost-author` — that bucket stays at 0 here because none of the 26
legacy rows are abandoned by a *removed* author, they just predate
the events table's `register_author` records. FR-016a (the seeded
`lost-author` identity exists for the case where it's needed) is
satisfied — the row is present in the `authors` table at
`00000000-0000-7000-8000-000000000002`.

### SC-005 — viewport Authors → memory first-click

PASS. Verified facts, events, and knowledges items.

### SC-006 / perf baseline (sprint 008 carry-over, refreshed)

PASS. Full corpus (`--facts 10000 --knowledge 50000`) seeded as the
`klams-bench` author through the REST surface — exercising the new
attribution path under load — then run 100 samples × 10 queries.
Headline: `memory_search` p95 = **146.9 ms** (full numbers in the
section above), well under the 1 s SC-006 ceiling. Bench corpus
was purged afterwards via the new author-based `just bench-clean`
(10 000 facts + ~60 000 knowledge points removed in a single
author-scoped DELETE; live store back to 27 facts / 6 knowledge
items / 3 events).
