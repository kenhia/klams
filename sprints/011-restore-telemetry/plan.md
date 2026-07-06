# Sprint 011 — Restore klams telemetry (Grafana "No Data")

**Status:** Active  
**Branch:** `011-restore-telemetry`  
**Date:** 2026-06-13  
**Type:** Lightweight fix sprint (no full Spec Kit flow)  
**Spans two repos:** `klams` (dashboard JSON + docs) and `ansible-k` (central
Prometheus scrape job — source of truth for kubsdb infra).

## Problem

The `klams` Grafana dashboard (`uid klams-006`, http://kubsdb:3000) renders but
every panel shows **"No Data"**. Diagnosed 2026-06-13.

## Root causes (diagnosed, ranked)

1. **Central Prometheus does not scrape klams at all** — THE blackout cause.
   The Prometheus behind Grafana (`kubsdb:9090`) runs jobs `prometheus / node /
   nvidia-gpu / unifi / hvsim` and holds **zero `klams_*` series**. There is no
   `klams-service` job. So even panels with correct queries
   (`klams_summarization_lag_seconds`, `klams_mcp_search_total`,
   `klams_mcp_writes_total`) have nothing to read.
   - Deployed config: `/datastore/prometheus/prometheus.yml` on kubsdb (compose at
     `/datastore/prometheus/docker-compose.yml`, `--web.enable-lifecycle` ON →
     hot reload via `POST http://kubsdb:9090/-/reload`).
   - The prometheus container maps `kubs0:192.168.1.235` in `extra_hosts`, so a
     target of `kubs0:7777` resolves. klams-service on kubs0:7777 is healthy and
     serving `/metrics`.
   - The repo's `deploy/prometheus/prometheus.yml` (target
     `host.docker.internal:7777`) is the klams-compose sample and is NOT what the
     live Grafana uses; `host.docker.internal` would also be wrong from kubsdb.

2. **Grafana datasource UID mismatch** — panels reference
   `{ "uid": "prometheus-default" }`; the real datasource UID is `prometheus`.

3. **Metric-name drift since sprint-006** — several panels query renamed/moved
   metrics. Live names confirmed from `http://kubs0:7777/metrics`:

   | Panel | Dashboard query | Live metric / label |
   | --- | --- | --- |
   | 1 Event queue depth | `klams_event_queue_depth` | `klams_queue_depth` |
   | 2 Worker utilization | `klams_worker_active` | `klams_workers_active` |
   | 3 Write throughput | `klams_http_requests_total{…}` by `route` | `axum_http_requests_total{…}` by `endpoint` |
   | 4 Search/context latency | `klams_http_request_duration_seconds_bucket{route=~…}` | `axum_http_requests_duration_seconds_bucket{endpoint=~…}` |
   | 5 Error rate | `klams_http_requests_total{status=~"5.."}` | `axum_http_requests_total{status=~"5.."}` |
   | 8 Summarization lag | `klams_summarization_lag_seconds` | ✅ unchanged |
   | 12 MCP writes | `klams_mcp_writes_total` | ✅ unchanged |
   | 14 MCP searches | `klams_mcp_search_total` | ✅ unchanged |
   | 6,7,9,10,11 backup/maintenance | `klams_backup_*`, `klams_maintenance_mode_active` | **service-exposed but lazy** — registered only after the first backup/maintenance event; correct names, no rename needed |

   Note: route-label panels (3,4,5) lose the `/memory/...` route grouping —
   `axum-prometheus` labels are `endpoint` (full path) + `status`, no `route`.

## Known side concerns (decide scope)

- **Backup / maintenance panels (6,7,9,10,11)** reference metrics defined in
  `klams-service` (`crates/klams-service/src/backup/metrics.rs`) but registered
  **lazily** via the `metrics` crate — they only appear in `/metrics` after the
  first backup/maintenance event since service start. **Decision (T006): keep the
  panels unchanged.** The metric names are correct; the panels read "No Data"
  only until the next backup runs, then populate automatically. No exporter wiring
  needed.
- **Prometheus template drift** — `ansible-k roles/prometheus/templates/
  prometheus.yml.j2` (34 lines) is missing the live `hvsim` job (and unifi is
  conditional). Adding klams ONLY to the deployed file would be lost on the next
  ansible run; adding ONLY to the template + re-running risks dropping `hvsim`.
  Reconcile: bring the template in sync with the deployed jobs **and** add klams.

## Tech / environment

- klams-service: kubs0:7777, `/metrics` (prometheus text). Live metric names per
  table above.
- Prometheus: container on kubsdb, config `/datastore/prometheus/prometheus.yml`,
  reload `POST http://kubsdb:9090/-/reload`. Source of truth: `~/ansible-k`
  `roles/prometheus`.
- Grafana: kubsdb:3000, datasource uid `prometheus`. API creds = `claude` user in
  repo-root `.env` (gitignored): `HV_GRAFANA_USERNAME/PASSWORD/URL`.
- Dashboard source in repo: `deploy/grafana/klams.json`.

## Acceptance

- Prometheus shows a healthy `klams-service` target and `klams_*` series.
- The klams Grafana dashboard renders live data on all non-backup panels.
- Dashboard JSON in the repo matches what's live (datasource uid + metric names).
- Prometheus scrape job persisted in ansible-k (not just hand-edited on kubsdb),
  with the template/deployed drift reconciled or explicitly noted.
- Docs updated (`docs/setup.md` or architecture monitoring section).
