# Contracts — Activity & Observability (sprint 008)

Authoritative interface surfaces for sprint 008. Each file in this
directory is the single source of truth for one interface; the
implementation (and its tests) MUST match these documents.

| File | Surface |
|------|---------|
| [mcp-event-search.md](./mcp-event-search.md) | `event_search` MCP tool reference + scope + output shape |
| [tool-schemas/event_search.json](./tool-schemas/event_search.json) | JSON Schema 2020-12 input schema for the tool |
| [rest-memories.md](./rest-memories.md) | `GET /v1/memories` HTTP contract |
| [error-codes.md](./error-codes.md) | New error codes added in sprint 008 (`WINDOW_TOO_LARGE`, `INVALID_WINDOW`) |
| [grafana-mcp-panels.md](./grafana-mcp-panels.md) | PromQL for the three "MCP author activity" Grafana panels |
| [prometheus-scrape.md](./prometheus-scrape.md) | `prometheus.yml` scrape job for `klams-service` |
| [bench-harness.md](./bench-harness.md) | CLI surface for `klams-bench` + `perf-baseline.md` output format |

The MCP tool envelope, scope model, cursor encoding, and `PublicMemory`
projection shape are inherited unchanged from sprint 007 — see
[../../007-mcp-server/contracts/](../../007-mcp-server/contracts/) for
those primitives.
