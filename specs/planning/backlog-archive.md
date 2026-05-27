# klams backlog archive

Items that have been moved to spec/sprint or cut from consideration.

## `event_search` MCP tool **PRIORITY**

[specs/008-activity-observability/spec.md](../008-activity-observability/spec.md)

Dedicated filter-based tool for event lookup. `memory_search` is
semantic/embedding-driven over free text — events have no text body,
only `category` + structured `payload`, so they are not surfaced via
`memory_search` (confirmed during sprint 007 quickstart walk).
Proposed shape:

```
event_search {
  author_id?: string,
  category?: string | string[],
  since?: timestamp,
  until?: timestamp,
  payload_match?: { key: value, ... },   // exact-equality on payload fields
  limit?: int (default 50, max 500),
  order?: "desc" | "asc" (default desc)
}
```

Read scope. Pure SQL — no embedding pipeline involved. Pick up
immediately after sprint 007 ships.

## Viewport activity-by-date-range tab **PRIORITY**

[specs/008-activity-observability/spec.md](../008-activity-observability/spec.md)

New top-level viewport tab that lists all memory activity within a
configurable time window, default last 24 hours. Same row-rendering
patterns as the author drilldown (kind badge, state, summary, tags,
deep-link per memory) but scoped by date rather than author so an
operator can quickly see "what's new in the memory store."

Suggested controls: from/to datetime pickers (default = now − 24 h),
kind filter (fact / knowledge / event / all), state filter
(live / soft-deleted / all), author filter (optional multi-select),
limit + pagination cursor.

Touch points:

- Server: likely a new `GET /v1/memories?since=&until=&kinds=&state=&authors=&limit=&cursor=`
  endpoint (or reuse `event_search` from the other PRIORITY item for
  events and add facts/knowledge listings). Pure SQL/Qdrant filter,
  no embedding pipeline.
- Viewport: new route (`/activity`) + nav link, Tauri command, type.
- Composes naturally with `event_search` — share the date-range
  paging primitive.

## Grafana "MCP author activity" panel: No Data **PRIORITY**

[specs/008-activity-observability/spec.md](../008-activity-observability/spec.md)

Quickstart §11: counters under `klams_mcp_*` are emitted correctly by
the service (verified via `curl /metrics`) but the Grafana dashboard
panels render "No Data". Likely a Prometheus scrape config drift or a
PromQL/label-name mismatch introduced when the per-author label set
landed in sprint 007. Blocks SC-005. Investigate Prometheus scrape
target + the panel queries in `deploy/grafana/`.

## SC-006 perf benchmark (T062, sprint 007)

[specs/008-activity-observability/spec.md](../008-activity-observability/spec.md)

Validate sprint-007 success criterion SC-006: load a fixture with
≥ 10k facts + 50k knowledge items, run `memory_search` 100×, and
record p95 latency. Attach the result to a follow-up PR or note it in
the next sprint's spec. Do **not** start tuning work if p95 exceeds
1 s — surface the measurement first and let the user decide whether
the actual overshoot is "good enough" for the homelab before any
optimization work begins.

Deferred from sprint 007 ship per user decision (2026-05-25).
