# Perf baseline — sprint 008

> Generated 2026-05-27T05:39:24.058420517Z by `just bench-run` on `kubs0`.
> Fixture seed: `0x0000c0ffee000008` · Store: 500 facts, 2,000 knowledge items.

## `memory_search` latency (100 samples across 10 queries)

| Metric    |              Value |
| --------- | -----------------: |
| p50       |       13.6 ms |
| p95       |       18.4 ms |
| p99       |       24.9 ms |
| min / max | 8.9 ms / 31.8 ms |
| mean      |      14.3 ms |

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
- **Smoke run**: corpus is below the canonical 10,000/50,000 target — rerun `just bench-seed && just bench-run` with the full corpus once kwi work item #26 is fixed.
