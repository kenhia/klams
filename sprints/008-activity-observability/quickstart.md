# Quickstart — Activity & Observability (sprint 008)

This walkthrough takes a fresh checkout of `008-activity-observability`
from zero to "operator sees activity and perf numbers" in under 30
minutes. It doubles as the acceptance script for the spec's five user
stories (US1–US5).

Prerequisites:

- Sprint 007 already shipped (see commit `<sprint-007-merge-sha>`); the
  `klams-service` binary builds, the MCP server mounts, the
  `authors` table exists, and at least one `register_author` call has
  succeeded against your store.
- `just`, `cargo`, `docker compose`, `curl`, `jq` available.
- A running test compose (`just compose-up-test`) or a configured
  `klams.toml` pointing at your own Postgres / Qdrant / TEI.
- One scoped token with at least `read` (for the HTTP endpoint, MCP
  tool, and viewport) and one with `write` (for the perf seed).

---

## 1. Build the workspace

```bash
just gate
```

Expected: `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, and `cargo test --workspace` all pass. The new `klams-bench` crate compiles as a workspace member.

---

## 2. Start the service

```bash
just service-run
```

Expected: same boot output as sprint 007. No new migrations apply. The new `GET /v1/memories` route and `event_search` MCP tool register at startup; `/healthz` is unchanged.

Verify the new route is visible:

```bash
curl -sf -H "Authorization: Bearer ${KLAMS_TOKEN}" \
     "http://localhost:7777/v1/memories?limit=1" | jq .
```

Expected: `{"memories": [...], "next_cursor": ...}` (live rows from the last 24h).

Verify the new MCP tool is visible in `tools/list`:

```bash
just mcp-call tools/list '{}' | jq '.tools[] | .name' | grep event_search
```

Expected: `"event_search"` present.

---

## 3. User Story 1 — `event_search` finds events by category and window

Seed a few events:

```bash
for cat in Deploy Deploy Maintenance Deploy; do
    just mcp-call tools/call \
        '{"name":"memory_append_event","arguments":{"author_id":"'"${AUTHOR_ID}"'","category":"'"$cat"'","payload":{"service":"widget"}}}'
done
```

Run an `event_search` filtered by category and window:

```bash
just mcp-call tools/call \
    '{"name":"event_search","arguments":{"category":"Deploy","since":"'"$(date -u -d '1 hour ago' +%FT%TZ)"'"}}' \
    | jq '.events | length, .next_cursor'
```

Expected: `3` events returned (the three `Deploy` writes), `null` cursor (window not exhausted past the page limit), each carrying `author.agent_name` and `author.model`.

**Acceptance**:

- US1 scenario 1 — only `Deploy` events returned within the window, newest first.
- US1 scenario 2 — same call with `"payload_match":{"service":"widget"}` returns the same 3.
- US1 scenario 4 — call succeeds with a `read`-only token.
- US1 scenario 5 — call with `since=until=now` returns `{"events": [], "next_cursor": null}`.

Verify the no-embedding contract:

```bash
just mcp-call tools/call \
    '{"name":"event_search","arguments":{"category":"Deploy"}}' &
sleep 1
curl -sf http://localhost:7777/metrics | grep -E 'tei_(requests|latency)' | head -3
```

Expected: TEI counters did NOT increment during the call (pure SQL — FR-004).

---

## 4. User Story 3 — `GET /v1/memories` cross-author listing

Seed writes from two distinct authors across all kinds (use any
combination of `memory_add` for facts/knowledge and
`memory_append_event` for events).

Default 24h window, all kinds, live state:

```bash
curl -sf -H "Authorization: Bearer ${KLAMS_TOKEN}" \
     "http://localhost:7777/v1/memories" | jq '.memories | length'
```

Expected: count matches the seeded total. Newest first by
`(created_at, id)` across all kinds.

Narrow by kind:

```bash
curl -sf -H "Authorization: Bearer ${KLAMS_TOKEN}" \
     "http://localhost:7777/v1/memories?kinds=event" \
     | jq '.memories[].kind' | sort -u
# expected: "event"
```

Filter by authors:

```bash
curl -sf -H "Authorization: Bearer ${KLAMS_TOKEN}" \
     "http://localhost:7777/v1/memories?authors=${AUTHOR_A},${AUTHOR_B}" \
     | jq '.memories | length'
```

Surface deleted rows:

```bash
# Soft-delete one row first via memory_delete, then:
curl -sf -H "Authorization: Bearer ${KLAMS_TOKEN}" \
     "http://localhost:7777/v1/memories?state=deleted" \
     | jq '.memories[0] | {state, deleted_at, deleted_by}'
```

Expected: the deleted row appears, with `state: "deleted"`, populated `deleted_at`, populated `deleted_by`.

Window-cap check:

```bash
curl -i -H "Authorization: Bearer ${KLAMS_TOKEN}" \
     "http://localhost:7777/v1/memories?since=2026-01-01T00:00:00Z&until=2026-05-01T00:00:00Z"
```

Expected: `400 Bad Request` with body
`{"error_code": "WINDOW_TOO_LARGE", "message": "...", "window_max_days": 30}`.

**Acceptance**: US3 scenarios 1–7 all pass.

---

## 5. User Story 2 — Viewport `/activity` tab

Launch the viewport (existing path; sprint 007 unchanged):

```bash
just viewport-dev    # Linux/WSLg dev mode
```

In the viewport:

1. Click the new "Activity" entry in the nav bar.
2. Confirm the page renders the last 24h of memory items across all authors and all kinds, newest first, with one row per item.
3. Toggle the "Kind" filter to "event"; confirm only events remain.
4. Toggle the "State" filter to "soft-deleted"; confirm only deleted rows appear.
5. Click any row; confirm navigation to the existing per-kind detail route (`/facts/:id`, `/knowledge/:id`, `/events/:id`).
6. Scroll past the page limit; confirm "next page" loads more rows without resetting the filters.

**Acceptance**: US2 scenarios 1–5 all pass.

---

## 6. User Story 4 — Grafana panel fix

Bring up the observability stack with the new Prometheus config:

```bash
docker compose -f deploy/docker-compose.yml --profile observability up -d prometheus
```

Confirm Prometheus is scraping `klams-service`:

```bash
curl -s "http://localhost:9090/api/v1/targets" | jq '.data.activeTargets[] | .labels.job'
# expected: includes "klams-service"
```

Drive at least one MCP write, one MCP delete, and one MCP search from
a registered author (any of the earlier steps suffice).

Restart Grafana so it re-reads the dashboard JSON:

```bash
docker restart klams-grafana    # or your local Grafana process
```

Open the klams Grafana dashboard. The three new "MCP author activity"
panels MUST render non-empty series within one Prometheus scrape
interval (default 15 s):

- "MCP writes by agent / model / kind"
- "MCP deletes by agent / model / mode"
- "MCP searches by agent / model"

**Acceptance**: US4 scenarios 1–3 all pass.

If a panel still shows "No Data": verify the corresponding `klams_mcp_*` counter is non-empty in `/metrics`, then check Prometheus's target page for scrape errors.

### Phase 6 verification log

- 2026-05-26: compose/profile wiring and dashboard JSON were validated in-repo (profile services present in `deploy/docker-compose.yml`, scrape file checked in at `deploy/prometheus/prometheus.yml`, dashboard JSON includes three MCP author activity panels).
- Runtime panel rendering verification remains an operator step against a live stack (quickstart commands above), because this repository validation run does not include a provisioned homelab deployment.

---

## 7. User Story 5a — Perf fixture

Use a token with `write` scope:

```bash
KLAMS_TOKEN_WRITE=... just bench-seed
```

Expected: at least 10,000 facts and at least 50,000 knowledge items
in the store after the run.

```bash
just db-psql -t -c "SELECT count(*) FROM facts;"
# expected: >= 10000
just db-psql -t -c "SELECT count(*) FROM events;"
curl -sf "http://localhost:6333/collections/knowledge_items" \
     | jq '.result.points_count'
# expected: >= 50000
```

Re-run the seed with the same seed and confirm row counts do not
balloon (dedupe absorbs the rewrites — corpus is deterministic, FR-019):

```bash
just bench-seed    # runs with default seed 0xC0FFEE_0008
just db-psql -t -c "SELECT count(*) FROM facts;"
# expected: same count as before
```

---

## 8. User Story 5b — Perf harness

```bash
just bench-run
```

Expected: the harness runs 100 calls against `memory_search` and writes
`sprints/008-activity-observability/perf-baseline.md`. Exit code 0
regardless of where p95 lands.

Open the file:

```bash
cat sprints/008-activity-observability/perf-baseline.md
```

Expected: header block with timestamp + host + seed + row counts;
a table of p50 / p95 / p99 / min / max / mean latency numbers; the
list of sample queries.

---

## 9. README discoverability

From a fresh `git pull`, open `README.md`. Within 30 seconds of opening
the file (SC-005), locate and click the link to the perf baseline:

```text
[Performance baseline](sprints/008-activity-observability/perf-baseline.md)
```

**Acceptance**: US5 scenario 3 passes.

---

## 10. Regression check — sprint 007 surfaces

Run the existing sprint 007 integration suite to confirm sprint 008 is
strictly additive (SC-006):

```bash
cargo test --workspace --test 'mcp_*' --test 'api_authors_*' --test 'auth_*'
```

Expected: all sprint 007 tests pass without modification. No removed
fields, no renamed routes, no changed error codes.

---

## Acceptance summary

| User Story | Step(s) | Acceptance scenarios |
|------------|---------|----------------------|
| US1 | §3 | 1, 2, 3, 4, 5 |
| US2 | §5 | 1, 2, 3, 4, 5 |
| US3 | §4 | 1, 2, 3, 4, 5, 6, 7 |
| US4 | §6 | 1, 2, 3 |
| US5 | §7, §8, §9 | 1, 2, 3, 4 |

All five user stories independently testable; the order above is
one workable sequence but each section stands alone given the prereqs
in §1 and §2.
