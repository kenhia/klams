# deploy/prometheus

Sprint 008 checked-in Prometheus scrape config for the homelab
klams instance.

Contract reference:
sprints/008-activity-observability/contracts/prometheus-scrape.md

## Files

- prometheus.yml: compose-side Prometheus config used by the
	observability profile in deploy/docker-compose.yml.

## Compose mode (repo default)

Start observability services:

```bash
docker compose --env-file /ai/klams/config/compose.env \
	-f deploy/docker-compose.yml \
	--profile observability up -d prometheus grafana
```

This profile mounts deploy/prometheus/prometheus.yml and scrapes
host.docker.internal:7777/metrics (the default klams-service port).

Linux hosts must allow host-gateway resolution; compose wiring includes:

- extra_hosts: host.docker.internal:host-gateway

## On-host Prometheus mode

If Prometheus is managed outside compose, copy this file into the host
Prometheus config and keep the klams-service scrape job equivalent.

## Quick validation

After starting the profile and generating at least one MCP write:

```bash
curl -s "http://localhost:9090/api/v1/query?query=klams_mcp_writes_total" | jq '.data.result | length'
```

Expected: value > 0.
