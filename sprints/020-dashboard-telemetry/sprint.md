# Sprint 020 — dashboard telemetry: no more "No Data"

**Branch:** `020-dashboard-telemetry`
**korg:** proposal 178 — WI #63 (last open klams item; #61/#62 shipped in 018)
**Type:** telemetry truthfulness + one real operational fix the No Data
panels were hiding.

## Goal

7 of 14 panels on the klams Grafana dashboard (kubsdb:3000, uid
`klams-006`) read "No Data". Triage (2026-07-09, via the Grafana API)
split them into four causes; fix each so every panel shows real data
or an honest zero.

| Panels | Cause |
|---|---|
| 4 Search+context latency | WI #63 — panel watches REST routes with no traffic; real retrieval flows over MCP |
| 6, 9, 10, 11 (backup family) | **Backups have failed nightly since May 31**: sprint 009's hardened unit (`ProtectSystem=strict`) never granted write access to `/gratch/klams-backup`; every run dies on `lockfile: io: Read-only file system (os error 30)`. Newest artifact: 2026-05-30. |
| 7 Maintenance mode | `set_maintenance_active(false)` runs at main.rs:66, but the global Prometheus recorder is only installed by `with_metrics` at ~line 228 — the gauge write is dropped before any recorder exists |
| 5 5xx rate, 13 MCP deletes | Legitimately zero — counter series don't exist until the first 5xx/delete; "No Data" where the truth is 0 |

## Scope

1. **Backups back on the air** (worth the sprint by itself):
   `ReadWritePaths=/gratch/klams-backup` in
   `deploy/klams-service.service` (homelab-specific path is fine per
   AGENTS.md — no generalizing). Deploy, run a live backup
   (`--run-backup-now` or wait for the 08:01 UTC window), confirm
   fresh artifacts + `klams_backup_*` series, panels 6/9/10/11
   populate. Consider seeding `klams_backup_last_success_timestamp_seconds`
   at startup from the newest artifact on disk so panel 6 survives
   restarts without waiting for the next run.
2. **WI #63 — retrieval latency where the traffic is**: one histogram
   (`klams_retrieval_duration_seconds{op, transport}`) recorded via a
   shared helper at the retrieval entry points — REST
   `/memory/search` + `/memory/context` (`transport="rest"`) and MCP
   `memory_search` (`transport="mcp"`; MCP has no context tool).
   Repoint panel 4 at it; update `deploy/grafana/klams.json` and the
   `grafana_dashboard_json.rs` series-coverage contract.
3. **Maintenance gauge**: make the recorder exist before the first
   metric write (install it early in main, or re-emit
   describes + gauge right after installation). Panel 7 shows 0.
4. **Honest zeros**: panels 5 and 13 get `noValue: "0"` in the
   dashboard JSON — the labels (status, agent/model/mode) can't be
   sensibly pre-registered server-side.
5. **Live dashboard sync + visual verify**: push the updated dashboard
   to kubsdb Grafana (API if the `claude` user may edit; otherwise
   hand to Ken), then verify each fixed panel via the Grafana
   **render API** (image renderer confirmed present — no playwright
   install needed on kubs0).

## Out of scope

- Playwright on kubs0 (render API covers visual verification).
- New alerting (a "backup stale" alert is tempting — file a WI if the
  appetite is there).
- ansible-k / k-homelab `klams-grafana.md` series-table handoff
  update happens in that repo; noted here, done there.

## Acceptance

- A fresh backup artifact pair exists in `/gratch/klams-backup` dated
  2026-07-09/10 and `klams_backup_runs_total{ok="true"}` is non-zero.
- `curl /metrics` right after a service restart shows
  `klams_maintenance_mode_active 0` and (seeded) last-success gauge.
- Panel-by-panel instant queries (the triage script) return data for
  panels 4, 6, 7, 9, 10, 11 after normal traffic; 5 and 13 render "0"
  instead of "No Data".
- `just gate` green; WI #63's contract (latency panel lights up under
  MCP-driven usage) verified live via render-API screenshots.

## Chronicle

- (2026-07-09) Opened from korg proposal 178 (only #63 remained).
  Triage via Grafana API (creds in `.env`, instance kubsdb:3000)
  found the four causes above — most notably 40 days of silent
  nightly backup failures (`journalctl -u klams-service | grep
  "backup run failed"` — every night since May 31, error 30 on the
  lockfile). The hardened unit shipped in sprint 009 on exactly
  May 31.
- (2026-07-09/10) Implemented + deployed `0.1.20`, all verified live:
  - **Backups are back.** With `ReadWritePaths` in place, a real
    scheduler run (window temporarily moved to 03:13 UTC, then
    restored to 08:01) produced `postgres-2026-07-10.dump` +
    `qdrant-2026-07-10.snapshot` — the first artifacts since May 30.
    `klams_backup_runs_total{ok="true"}` 1, last-success gauge live.
  - Maintenance gauge renders `0` immediately after restart
    (recorder-order fix), and the last-success gauge seeds from the
    newest on-disk `postgres-*.dump` mtime.
  - MCP `memory_search` traffic records
    `klams_retrieval_duration_seconds{op="search",transport="mcp"}`.
  - **Surprise #1:** the Grafana render API answers 200 but returns a
    "renderer not installed" placeholder PNG — no image renderer on
    kubsdb after all, so panel verification is data-level (instant +
    range queries via the API), not screenshots. Playwright still not
    needed.
  - **Surprise #2 (the deep one):** `metrics-exporter-prometheus`
    renders histograms WITHOUT configured buckets as Prometheus
    **summaries** (quantile label) — `_bucket` series never existed
    for ANY `klams_*` histogram, so panel 9's
    `klams_backup_duration_seconds_bucket` query has been structurally
    dead since sprint 006 (axum-prometheus only configures buckets for
    its own request-duration metric). Rather than rebuild the recorder
    with per-family bucket boundaries, panels 4 and 9 query the
    summary quantiles directly (`{quantile=~"0.5|0.95|0.99"}`) —
    exact quantiles, zero recorder surgery, fine for a single-instance
    service. The ansible-k handoff series table was corrected to match
    (rows now say summary, not `_bucket`).
  - Panel 11 (`status_hook`, feature not configured in prod) and
    panels 5/13 get `noValue: "0"` — zero is the truth there.
  - Dashboard pushed to kubsdb via API (v6). Final range-query triage
    over the 6h window: **all 14 panels** either show real data (10)
    or an explicit "0" (4). Zero "No Data".
