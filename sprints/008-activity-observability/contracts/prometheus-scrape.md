# Prometheus — scrape config for `klams-service` (sprint 008)

Authoritative scrape job for `klams-service`'s `/metrics` endpoint.
Sprint 008 adds this file at `deploy/prometheus/prometheus.yml` so a
clean checkout reproduces the Grafana panels (FR-018).

---

## File layout

```text
deploy/prometheus/
├── prometheus.yml            # this contract
└── README.md                 # operator-facing notes (deployment modes)
```

`README.md` describes two deployment modes (compose-side Prometheus
vs on-host Prometheus); only the compose-side mode is wired by the
existing `deploy/docker-compose.yml`. On-host deployments are
documented but not auto-wired.

---

## `prometheus.yml` (authoritative shape)

```yaml
global:
  scrape_interval: 15s
  evaluation_interval: 15s
  external_labels:
    instance: klams

scrape_configs:
  - job_name: klams-service
    metrics_path: /metrics
    scheme: http
    static_configs:
      - targets:
          # Default for systemd-deployed klams-service on the host:
          - host.docker.internal:7777
        labels:
          service: klams-service

  # Optional: scrape klams-monitor's exporter when running. The
  # monitor exposes /metrics on the same port pattern as klams-service.
  # Uncomment when klams-monitor is enabled in compose.
  # - job_name: klams-monitor
  #   metrics_path: /metrics
  #   static_configs:
  #     - targets: ["klams-monitor:7780"]
```

**Notes**:

- `host.docker.internal` is the canonical Docker bridge name for the host loopback. On Linux compose hosts that name resolves only when `extra_hosts: ["host.docker.internal:host-gateway"]` is added to the Prometheus service in `docker-compose.yml`. The `README.md` documents this.
- The scrape target port (`7777`) MUST match `[server].port` in `klams.toml` (the default).
- `external_labels.instance = klams` carries through to every emitted series so multi-instance Prometheus setups can distinguish klams from neighboring deployments.

---

## Compose wiring (additive change to `deploy/docker-compose.yml`)

Add a Prometheus service to the compose file, gated behind a profile
so existing operators who run Prometheus on-host don't pull a
second copy:

```yaml
services:
  prometheus:
    profiles: ["observability"]
    image: prom/prometheus:${PROMETHEUS_IMAGE_TAG:-v2.55.0}
    container_name: klams-prometheus
    restart: unless-stopped
    networks:
      klams-net:
        aliases: [prometheus]
    extra_hosts:
      - "host.docker.internal:host-gateway"
    ports:
      - "127.0.0.1:9090:9090"
    volumes:
      - ./prometheus/prometheus.yml:/etc/prometheus/prometheus.yml:ro
      - ${KLAMS_DATA_ROOT}/prometheus:/prometheus
```

Started with:

```bash
docker compose -f deploy/docker-compose.yml --profile observability up -d
```

The profile keeps the default `docker compose up -d` invocation
unchanged for operators who already run their own Prometheus.

---

## Acceptance check (matches SC-003)

After applying this config and a `docker compose ... up -d
prometheus`, the following query against Prometheus's API MUST return
at least one series after a single MCP write call from a registered
author:

```bash
curl -s "http://localhost:9090/api/v1/query?query=klams_mcp_writes_total" | jq '.data.result | length'
# expected: > 0
```

---

## What we deliberately do **not** add

- **Recording rules** — premature; raw counter rates are inexpensive.
- **Alerting rules** — out of scope (sprint adds visibility, not alerting).
- **Remote write / federation** — Ken's existing observability stack handles long-term storage on-host; the in-compose Prometheus is a read-only scratch instance.
