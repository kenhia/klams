# Tasks — Sprint 011: Restore klams telemetry

Branch: `011-restore-telemetry` · Plan: [plan.md](plan.md)

Legend: `[ ]` todo · `[X]` done. Tasks tagged by repo: **(klams)** or
**(ansible-k)**.

## US1 — Prometheus scrapes klams (unblocks everything)

- [X] T001 (ansible-k) Add a `klams-service` scrape job to
  `roles/prometheus/templates/prometheus.yml.j2` targeting `kubs0:7777`
  (metrics_path `/metrics`), and reconcile drift: add the live `hvsim` job to the
  template so a future ansible run does not drop it.
  → Staged as a handoff in `~/ansible-k/incoming/klams-telemetry-restore.md`
  (operator integrates; no playbook run).
- [X] T002 (kubsdb) Apply the job to the deployed
  `/datastore/prometheus/prometheus.yml` and hot-reload Prometheus
  (`POST http://kubsdb:9090/-/reload`). Prefer running the ansible prometheus role;
  fall back to a direct edit + reload if running the playbook is out of scope now.
  → Direct edit + hot-reload (backed up to `.bak-011-*`); reload returned 200.
- [X] T003 Verify: `klams-service` target is `up` in
  `http://kubsdb:9090/api/v1/targets` and `klams_*` series exist
  (`/api/v1/label/__name__/values`). → target `up`, 13 `klams_*` series present.

## US2 — Dashboard queries match live metrics

- [X] T004 (klams) Fix `deploy/grafana/klams.json` datasource UID
  `prometheus-default` → `prometheus` on all panels.
- [X] T005 (klams) Rename drifted metrics in the dashboard:
  - P1 `klams_event_queue_depth` → `klams_queue_depth`
  - P2 `klams_worker_active` → `klams_workers_active`
  - P3 `klams_http_requests_total` → `axum_http_requests_total`, `route` →
    `endpoint`
  - P4 `klams_http_request_duration_seconds_bucket` →
    `axum_http_requests_duration_seconds_bucket`, `route` → `endpoint`
  - P5 `klams_http_requests_total` → `axum_http_requests_total`
- [X] T006 (klams) Decide backup/maintenance panels (6,7,9,10,11): **keep
  unchanged** — metrics are service-defined but lazily registered (appear after
  the first backup/maintenance event). Recorded in the plan.
- [X] T007 (klams) Push the updated dashboard to Grafana via API (claude creds);
  confirm panels render live data. Bump dashboard `version`. → pushed (Grafana
  version 3); live queries return series. Updated the dashboard contract test
  (`grafana_dashboard_json.rs`) datasource assertion to `prometheus`.

## US3 — Persist & document

- [X] T008 (klams) Update repo `deploy/prometheus/prometheus.yml` sample to a
  correct, representative klams job (target note for kubs0 vs compose) so the repo
  sample is not misleading.
- [X] T009 (klams) Update docs (`docs/setup.md` monitoring/observability section)
  with how Prometheus scrapes klams and where the dashboard lives.
- [ ] T010 Final verify + ship: dashboard live, `just gate` green (if any Rust
  touched — likely none), commit, ship sprint.

## Checkpoint

Dashboard renders live data on all non-backup panels; scrape job persisted in
ansible-k; drift reconciled or noted.
