# klams Prometheus series contract

The authoritative list of `klams_*` series this service exposes on
`GET /metrics`. Two things are checked against it by
`crates/klams-service/tests/grafana_dashboard_json.rs`:

1. every series referenced by a panel in `klams.json` appears here, and
2. every series declared in `crates/*/src/**` appears here.

So adding a metric to the code, or a panel to the dashboard, without
documenting it here fails the gate.

**Sprint 032 (#680) moved this table into klams.** It used to live in
`ansible-k/specs/klams-integration/klams-grafana.md`, a repo that has
been inert since 2026-07-05 (its `justfile` is renamed `STOP-justfile`;
`k-homelab` is the devops owner now). The test read it from
`$HOME/ansible-k/...` and **self-skipped when absent**, so on any
machine without that checkout — including CI — the cross-check silently
did nothing while reporting green. Adding the two sprint-027 series
meant editing a deprecated repo to make a klams test pass. The series a
service emits and the dashboard that graphs them belong in the repo
that emits them.

Rows are the contract; the prose after each table is context.

## Write path

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_writes_total` | counter | `type` | 003 |
| `klams_writes_accepted_total` | counter | `type ∈ {fact, event, knowledge}` | 003 |
| `klams_writes_failed_total` | counter | `type`, `reason ∈ {too_large, embedding_rejected, embedding_unavailable, backend_unavailable, conflict, gone, backend, queue_full}` | 027 |
| `klams_write_latency_seconds` | histogram | `type` | 003 |
| `klams_validation_rejections_total` | counter | `reason` | 003 |
| `klams_version_conflicts_total` | counter | _none_ | 002 |
| `klams_dissents_total` | counter | _none_ | 002 |

## Queue and workers

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_queue_depth` | gauge | _none_ | 003 (renamed 006) |
| `klams_queue_capacity` | gauge | _none_ | 011 |
| `klams_workers_active` | gauge | _none_ | 003 (renamed 006) |

`klams_queue_depth` is resampled on a 2s timer, not only on write, so it
tracks worker drain rather than the last enqueue (sprint 011).

## Retrieval

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_retrieval_duration_seconds` | summary (quantile label) | `op ∈ {search, context, rerank}`, `transport ∈ {rest, mcp}` | 020 (`rerank` added 030) |
| `klams_search_misses_total` | counter | `reason ∈ {zero_hit, low_score}` | 021 |
| `klams_hybrid_source_contribution_total` | counter | `source` | 024 |
| `klams_rerank_skipped_total` | counter | _none_ | 030 |
| `klams_context_section_items_total` | counter | `section` | 005 |
| `klams_embedding_latency_seconds` | histogram | _none_ | 003 |

The klams histograms are emitted by the exporter as **summaries with a
`quantile` label**, not as `_bucket` series — corrected in sprint 020
after alerts written against `histogram_quantile()` matched nothing.

`klams_rerank_skipped_total` nonzero means the `klams-reranker`
container is sick and searches are being served un-reranked; it is not
a search-path failure (sprint 030).

## MCP surface

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_mcp_writes_total` | counter | `agent_name`, `model`, `kind ∈ {fact, event, knowledge}` | 008 |
| `klams_mcp_deletes_total` | counter | `agent_name`, `model`, `mode ∈ {hard, soft, restored}` | 008 |
| `klams_mcp_search_total` | counter | `agent_name`, `model` | 008 |
| `klams_mcp_oversize_writes_total` | counter | `agent_name` | 027 |

## Backup and maintenance

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_backup_last_success_timestamp_seconds` | gauge | _none_ | 006 |
| `klams_backup_duration_seconds` | summary (quantile label) | `kind ∈ {postgres, qdrant}` | 006 |
| `klams_backup_runs_total` | counter | `ok ∈ {true, false}` | 006 |
| `klams_backup_hook_invocations_total` | counter | `event`, `ok` | 006 |
| `klams_backup_dir_writable` | gauge | _none_ | 032 |
| `klams_maintenance_mode_active` | gauge | _none_ | 006 |

`klams_backup_dir_writable` is set once at startup by a
create/write/remove probe of `[backup].backup_dir`. `0` means every
backup run will fail — usually `ProtectSystem=strict` without a
matching `ReadWritePaths=` in the unit, the failure sprint 020 fixed
after it ran silently for two months (sprint 032, #647).

## Decay

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_decay_runs_total` | counter | _none_ | 002 |
| `klams_decay_facts_updated_total` | counter | _none_ | 002 |
| `klams_decay_config_reloads_total` | counter | _none_ | 002 |
| `klams_last_used_bumps_dropped_total` | counter | _none_ | 024 |

Note the plural: `klams_decay_config_reloads_total`. Docs have carried
`klams_decay_config_reload_total` — off by an `s`, so an alert copied
from them matches nothing (sprint 032, #648).

## Summarization

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_summarization_runs_total` | counter | `mechanism` | 005 |
| `klams_summarization_lag_seconds` | gauge | _none_ | 005 |

`mechanism` is always `extractive` from sprint 032 on — the LLM path
that could label it `llm` never generated anything and was removed
(#647/#335).

## Scanner

Emitted by `klams-scanner`, not `klams-service`; they reach Prometheus
via the scanner's own exposition, so they are **not** expected in a
`klams-service` `/metrics` scrape.

| Series | Type | Labels | Sprint |
|---|---|---|---|
| `klams_scanner_files_processed_total` | counter | _none_ | 010 |
| `klams_scanner_files_skipped_total` | counter | `reason` | 010 |
| `klams_scanner_chunks_indexed_total` | counter | _none_ | 010 |
| `klams_scanner_chunk_retries_total` | counter | _none_ | 022 |
| `klams_scanner_last_run_timestamp_seconds` | gauge | _none_ | 010 |

## Not klams-owned

`axum_http_requests_total` and
`axum_http_requests_duration_seconds_bucket` come from
`axum-prometheus` and are not in this contract; `klams_http_requests_total`
is the pre-006 name kept as a constant for compatibility.

## Series that do NOT exist

Documented at some point, never emitted. Alerts copied from those docs
match nothing (sprint 032, #648):

- `klams_mcp_scope_denied_total`
- `klams_tei_requests_total`
- `klams_mcp_calls_total{token_label}` — no `token_label` dimension
  exists on any series, which is why WI #670 had to establish legacy-
  token usage by auditing configs rather than by querying Prometheus.
- `klams_decay_config_reload_total` — see the decay section.
