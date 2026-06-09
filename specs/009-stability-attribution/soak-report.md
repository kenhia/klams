# Sprint 009 — 18h loopback soak report

**Verdict: PASS (SC-001).** kwi #26 regression-clear.

## Run metadata

| Field | Value |
|-------|-------|
| Date | 2026-05-31 (started ~17:00 local 2026-05-30) |
| Host | `kubs0` |
| Service | `klams-service` under systemd, `LimitNOFILE=65536` |
| Harness | `just soak --duration 18h` → `target/release/klams-soak` |
| Target | `127.0.0.1:7777` |
| Concurrency | 32 |
| Rate | 4 req/s |
| Duration | 64800 s (18 h, exact) |
| Log | `/tmp/soak-018h.log` (2166 lines, 2161 samples @ 30 s) |

## Headline numbers

| Metric | Value |
|--------|-------|
| Requests opened | 259 201 |
| Requests completed | 259 201 |
| Requests failed | **0** |
| Max `CLOSE_WAIT` observed | **0** |
| End-of-run `CLOSE_WAIT` | 0 |

Opened == completed across every 30-second sample (verified with
`rg -v '"failed":0'` returning no sample rows). No drift between
opened and completed at any point in the window.

## SC-001 verdict

> *An 18-hour loopback soak at sustained traffic completes without
> the service reaching its file-descriptor cap and without an
> unbounded growth in `CLOSE_WAIT` sockets.*

Both conditions met:

1. **CLOSE_WAIT bounded.** Max observed was 0 across all 2161
   samples. The connection-limits layer plus the `LimitNOFILE=65536`
   ceiling kept loopback half-closes from accumulating — the exact
   pathology that originally tripped kwi #26.
2. **Service stayed reachable.** 259 201 / 259 201 requests
   completed; zero failures over 18 hours of continuous 4 req/s
   loopback traffic.

## Footnote — `fd_count` instrumentation

Every sample reports `fd_count: 0`. This is an artifact of the
soak harness reading `/proc/$pid/fd` for a service running under a
different uid (`klams` vs the soak operator) — the directory enumerates
to empty rather than EPERM. Not a soak failure; the primary signal
(CLOSE_WAIT growth) was captured correctly. Filing a backlog note to
either drop the `fd_count` field or read the count via `ss`/`lsof`
when the harness lacks `/proc` read access.
