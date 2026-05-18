# klams-service operator guide

This document covers the runtime behaviour of `klams-service`:
authentication, health/metrics endpoints, exit codes, and the
recommended systemd deployment. For HTTP request/response shapes
see [specs/001-initial-mvp/contracts/openapi.yaml](../specs/001-initial-mvp/contracts/openapi.yaml).

## Authentication

All `/memory/*` routes require a bearer token. The token is loaded
from configuration (`auth.bearer_token` in the service config or
`KLAMS_AUTH_BEARER_TOKEN` env var) and compared in constant time.

```text
Authorization: Bearer <token>
```

`/healthz` and `/metrics` are **unauthenticated** so probes and
scrapers don't need credentials.

## Health endpoint

`GET /healthz` returns a `HealthSnapshot` JSON document and a
status code matching the aggregate state:

| Aggregate | HTTP | Meaning |
|-----------|------|---------|
| `Ok`       | 200  | All subsystems healthy. |
| `Degraded` | 503  | At least one subsystem reports `Degraded`, none are `Down`. |
| `Down`     | 503  | At least one subsystem is `Down`. |

Subsystems probed: Postgres (`SELECT 1`), Qdrant (collection list),
embedder (`GET {tei_url}/health`). Each probe result is cached for
~2s to absorb scrape storms (Kubernetes liveness/readiness, dashboards).

Sample (healthy) response:

```json
{
  "status": "Ok",
  "postgres":   { "state": "Ok" },
  "qdrant":     { "state": "Ok" },
  "embeddings": { "state": "Ok" },
  "queue":      { "depth": 0, "capacity": 256, "workers": 2 },
  "version":    "0.1.0",
  "uptime_seconds": 1234
}
```

The Rust client surfaces this via [`klams_client::Client::health`](../crates/klams-client/src/lib.rs)
which deserializes the snapshot regardless of whether the HTTP
status was 200 or 503, letting callers display degraded subsystems
instead of just a transport error.

## Metrics endpoint

`GET /metrics` exposes the Prometheus text exposition format,
including the standard axum-prometheus HTTP histograms and the
named klams metrics below.

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `klams_queue_depth` | gauge | – | Current queued write jobs. |
| `klams_queue_capacity` | gauge | – | Configured queue capacity. |
| `klams_workers_active` | gauge | – | Active worker tasks. |
| `klams_writes_accepted_total` | counter | `type=fact\|event\|knowledge` | Writes accepted onto the queue. |
| `klams_writes_failed_total` | counter | `type`, `reason=queue_full\|store_error\|too_large` | Writes rejected or failed downstream. |
| `klams_write_latency_seconds` | histogram | `type` | Handler-side latency from validation to enqueue completion. |
| `klams_search_latency_seconds` | histogram | – | Latency of `POST /memory/search`. |
| `klams_embedding_latency_seconds` | histogram | – | Latency of a single TEI call. |

## Exit codes

| Code | Cause |
|------|-------|
| `0`  | Clean shutdown (SIGTERM/SIGINT after `tokio::signal`). |
| `1`  | Configuration error (missing/invalid `klams-service.toml` or env). |
| `2`  | Dependency error at boot (Postgres unreachable, Qdrant connect failed, embedder schema mismatch). |
| `64` | Invalid CLI arguments (per `sysexits.h` convention). |

## systemd

Suggested unit (`/etc/systemd/system/klams.service`):

```ini
[Unit]
Description=klams memory service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=klams
Environment=KLAMS_CONFIG=/etc/klams/service.toml
ExecStart=/usr/local/bin/klams-service
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Operate with:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now klams.service
sudo systemctl status klams.service
journalctl -u klams.service -f      # tail logs
journalctl -u klams.service --since "1 hour ago"
```

## Reference

- HTTP contract: [openapi.yaml](../specs/001-initial-mvp/contracts/openapi.yaml)
- Spec: [spec.md](../specs/001-initial-mvp/spec.md) (US5 — Observability & Operations)
- Functional requirements FR-017..FR-020 cover `/healthz`, `/metrics`,
  exit codes, and journal output.
