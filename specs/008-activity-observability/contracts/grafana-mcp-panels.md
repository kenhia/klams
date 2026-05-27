# Grafana — "MCP author activity" panels (sprint 008)

Authoritative PromQL for the three panels added to
`deploy/grafana/klams.json` in this sprint. The label set on each
query MUST match what `crates/klams-mcp/src/metrics.rs` emits — see
[research.md R-006](../research.md#r-006--grafana-panel-failure-root-cause).

Panel JSON is appended to the existing dashboard's `panels` array.
Datasource UID matches the existing klams panels (the dashboard's
default Prometheus datasource).

---

## Panel 1 — MCP writes by agent / model / kind

**Title**: `MCP writes by agent / model / kind (req/min)`

**Description**: Per-author write throughput broken down by kind.

**PromQL**:

```promql
sum by (agent_name, model, kind) (
  rate(klams_mcp_writes_total[5m])
) * 60
```

**Legend**: `{{agent_name}} · {{model}} · {{kind}}`

**Visualization**: time-series, stacked bars.

---

## Panel 2 — MCP deletes by agent / model / mode

**Title**: `MCP deletes by agent / model / mode (req/min)`

**Description**: Soft / restored / hard delete throughput per author.

**PromQL**:

```promql
sum by (agent_name, model, mode) (
  rate(klams_mcp_deletes_total[5m])
) * 60
```

**Legend**: `{{agent_name}} · {{model}} · {{mode}}`

**Visualization**: time-series, stacked bars. Color `mode = hard` red, `mode = soft` yellow, `mode = restored` green.

---

## Panel 3 — MCP searches by agent / model

**Title**: `MCP searches by agent / model (req/min)`

**Description**: Search call rate per author. The request's `kinds`
filter is intentionally not a label (R-010 from sprint 007).

**PromQL**:

```promql
sum by (agent_name, model) (
  rate(klams_mcp_search_total[5m])
) * 60
```

**Legend**: `{{agent_name}} · {{model}}`

**Visualization**: time-series, lines.

---

## Acceptance check (matches SC-003)

Run quickstart §6 (drive at least one write, one delete, one search
from a registered author). After the next Prometheus scrape:

```promql
klams_mcp_writes_total{agent_name!=""}
klams_mcp_deletes_total{agent_name!=""}
klams_mcp_search_total{agent_name!=""}
```

— each MUST return non-empty series. If any panel still shows
"No Data", the failure mode is upstream of Grafana (scrape config
or the Prometheus instance itself); see
[prometheus-scrape.md](./prometheus-scrape.md).

---

## What we deliberately do **not** add

- **`klams_mcp_*` by `author_id`** — would explode cardinality. Per-author drilldown lives at the viewport `/authors/{id}` route (sprint 007).
- **A `kinds` label on `klams_mcp_search_total`** — combinatorial in the request's `kinds` set; sprint 007 R-010 prohibits.
- **Alerts** — out of scope; this sprint adds visibility, not alerting policy.
