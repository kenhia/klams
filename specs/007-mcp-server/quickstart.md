# Quickstart — MCP Memory Server (sprint 007)

This walkthrough takes a fresh checkout of `007-mcp-server` from
zero to "GHCP can store and retrieve a memory" in under 15 minutes
(SC-007). It doubles as the acceptance script for the spec's user
stories.

Prerequisites:

- Sprint 006 already shipped (see commit `782bcd9`); `klams-service`
  builds and runs locally.
- `just`, `cargo`, `docker compose` available.
- A running test compose (`just compose-up-test`) or a configured
  production-ish `klams.toml` pointing at your own Postgres / Qdrant.

---

## 1. Apply migrations

```bash
just db-migrate
```

Expected: `0005_authors_table.sql`, `0006_facts_author_and_soft_delete.sql`,
`0007_events_author.sql` apply successfully. Idempotent on rerun.

Verify the system author seed:

```bash
just db-psql -c "SELECT id, agent_name FROM authors WHERE id = '00000000-0000-7000-8000-000000000001';"
```

Expected: one row `(00000000-0000-7000-8000-000000000001, system)`.

---

## 2. Configure scoped tokens

Edit your `klams.toml`:

```toml
[auth]
# Keep the existing single token (still works, grants all scopes):
bearer_token = "your-existing-token"

# Add scoped tokens for new clients:
[[auth.tokens]]
token = "ghcp-write-XXXXXXXXXXXXXXXX"
scopes = ["read", "write"]
label = "ghcp"

[[auth.tokens]]
token = "viewport-readonly-XXXXXXXXXXXX"
scopes = ["read"]
label = "viewport"

[[auth.tokens]]
token = "ken-admin-XXXXXXXXXXXXXXXXXX"
scopes = ["read", "write", "admin"]
label = "ken-admin"
```

Validate without starting the server:

```bash
just service-validate-config
```

Expected: exit 0; any warnings (e.g., short token, missing label) printed to stderr.

---

## 3. Start the service

```bash
just service-run
```

Expected: `klams-service` boots, applies the Qdrant payload backfill (first run only), and logs

```text
INFO klams_mcp: MCP server mounted at /mcp (transports: streamable_http, http_sse)
INFO klams_service: ready on 0.0.0.0:8088
```

Probe:

```bash
curl -s http://localhost:8088/healthz | jq .
```

Expected: `{"status":"ok", ..., "mcp":{"enabled":true,"transports":["streamable_http","http_sse"]}}`.

---

## 4. Register an MCP client — VS Code path

Create `<workspace>/.vscode/mcp.json` (or add to an existing one — see
[/home/ken/src/ai/klams/.vscode/mcp.json](../../.vscode/mcp.json) for the file shape Ken
already uses for `github`, `kwi`, and `kpidash`):

```jsonc
{
  "servers": {
    "klams": {
      "type": "http",
      "url": "http://kubs0:8088/mcp",
      "headers": {
        "Authorization": "Bearer ghcp-write-XXXXXXXXXXXXXXXX"
      }
    }
  }
}
```

Reload the VS Code window. The status bar should show the klams MCP
server connected; GHCP's tool palette should list the klams tools.

---

## 4b. Register an MCP client — GHCP CLI path

Edit `~/.copilot/mcp-config.json`:

```jsonc
{
  "mcpServers": {
    "klams": {
      "type": "http",
      "url": "http://kubs0:8088/mcp",
      "headers": {
        "Authorization": "Bearer ghcp-write-XXXXXXXXXXXXXXXX"
      },
      "tools": ["*"]
    }
  }
}
```

Restart the GHCP CLI process. List tools to confirm:

```bash
copilot mcp tools klams
```

Expected: `register_author`, `memory_add`, `memory_search`,
`memory_related`, `memory_delete`, `memory_append_event` (write +
read scopes). No `memory_admin_*` tools visible (this token lacks
admin scope — that's the FR-020 filter at work).

---

## 5. User story 1 walkthrough — write a fact via MCP

In a GHCP session (VS Code or CLI):

```text
> Use klams to record that I prefer just over make for new repos.
```

GHCP should call `register_author` first, then `memory_add` with
`kind: "fact"`. Verify from a separate terminal:

```bash
just db-psql -c "SELECT a.agent_name, a.model, f.payload
                 FROM facts f JOIN authors a ON f.author_id = a.id
                 WHERE a.agent_name != 'system'
                 ORDER BY f.created_at DESC LIMIT 1;"
```

Expected: one row, the fact's payload visible, agent_name = "GHCP",
model populated.

---

## 6. User story 2 walkthrough — read back via MCP

In a *different* GHCP session (different VS Code window):

```text
> What do I prefer for new-repo build automation?
```

GHCP should call `memory_search`. Confirm the response contains the
fact with author attribution and no internal fields (no
`decay_weight`, no `version`, no raw embedding vector).

CLI probe equivalent (curl against the MCP endpoint with a Streamable
HTTP envelope is awkward; easier to spot-check via the new viewport
route below or via `just mcp-call`):

```bash
just mcp-call memory_search '{"query": "build automation new repo", "top_k": 5}'
```

---

## 7. User story 3 walkthrough — append an event

From a controller-driven script or via `just mcp-call`:

```bash
AUTHOR_ID=$(just mcp-call register_author '{"agent_name":"controller","model":null,"repo":"/home/ken/src/ansible-k"}' | jq -r .author_id)
just mcp-call memory_append_event "$(jq -n --arg a "$AUTHOR_ID" '{
  author_id: $a,
  category: "Deploy",
  payload: {service: "widget", host: "kub3", version: "1.4.2"}
}')"
```

Verify via `memory_search` with `kinds: ["event"]`:

```bash
just mcp-call memory_search '{"query":"widget deploy","kinds":["event"],"top_k":5}'
```

Expected: event row with author attribution.

---

## 8. User story 4 walkthrough — soft delete and restore

Capture an existing memory id (from §5):

```bash
ID=$(just db-psql -t -c "SELECT id FROM facts ORDER BY created_at DESC LIMIT 1" | tr -d ' ')
```

Soft-delete with the write token:

```bash
just mcp-call memory_delete "$(jq -n --arg id "$ID" '{id: $id}')"
```

Confirm it's hidden from search:

```bash
just mcp-call memory_search '{"query":"just make build","top_k":5}' | jq '.results | length'
# Expected: 0
```

Switch to the admin token (set `KLAMS_TOKEN=ken-admin-...` in your
shell) and list deleted items:

```bash
just mcp-call memory_admin_list_deleted '{"kinds":["fact"],"limit":10}'
```

Restore:

```bash
just mcp-call memory_admin_restore "$(jq -n --arg id "$ID" '{id: $id}')"
```

Confirm the item is back:

```bash
just mcp-call memory_search '{"query":"just make build","top_k":5}' | jq '.results | length'
# Expected: 1
```

---

## 9. User story 4 — scope enforcement spot-check

With the **read-only** token:

```bash
KLAMS_TOKEN=viewport-readonly-... just mcp-call memory_delete '{"id":"00000000-0000-0000-0000-000000000000"}'
```

Expected: `{"isError":true,"_meta":{"error_code":"INSUFFICIENT_SCOPE"}, ...}`.

With the **write** (non-admin) token:

```bash
just mcp-call memory_admin_restore '{"id":"00000000-0000-0000-0000-000000000000"}'
```

Expected: `{"isError":true,"_meta":{"error_code":"INSUFFICIENT_SCOPE"}, ...}`.

And from `tools/list`, only the appropriate subset should be visible
per token.

---

## 10. User story 5 walkthrough — viewport author drilldown

Build / run the viewport against this service:

```bash
cd viewport
pnpm install
pnpm tauri dev
```

Navigate to `/authors`. Expected: one row per registered author with
`agent_name`, `model`, `session_title`, `last_seen_at`, and counts.
Click a row → see the per-author memories list with `live` /
`soft-deleted` state badges.

---

## 11. Grafana panel sanity-check

Scrape `/metrics`:

```bash
curl -s http://localhost:8088/metrics | grep '^klams_mcp_'
```

Expected counters (after exercising §5–§8):

```text
klams_mcp_writes_total{agent_name="GHCP",model="claude-opus-4.7",kind="fact",outcome="ok"}        1
klams_mcp_writes_total{agent_name="controller",model="",kind="event",outcome="ok"}                 1
klams_mcp_deletes_total{agent_name="GHCP",model="claude-opus-4.7",mode="soft"}                     1
klams_mcp_deletes_total{agent_name="ken-admin",model="",mode="restored"}                           1
klams_mcp_search_total{agent_name="GHCP",model="claude-opus-4.7",kinds="fact"}                     2
```

Confirm via the klams Grafana dashboard that the "MCP author activity"
panel renders the breakdown.

---

## 12. Restore-from-rogue-agent drill

Script `tests/integration/mcp_rogue_agent.rs` (or the manual rehearsal
documented in `docs/usage.md` after Phase 6) executes:

1. Seed 100 facts via fixtures.
2. Register a rogue author.
3. Call `memory_delete` on all 100 ids.
4. Assert `memory_search` returns zero items.
5. Switch to admin scope.
6. Call `memory_admin_list_deleted` → 100 rows.
7. Call `memory_admin_restore` on each id.
8. Assert `memory_search` returns 100 items again, contents identical.

Pass criteria for SC-008: zero rows lost from Postgres / Qdrant.

---

## Success criteria checklist

After completing the walkthrough:

- [ ] SC-001 — §5 + §6 succeeded end-to-end
- [ ] SC-002 — `SELECT COUNT(*) FROM facts WHERE author_id IS NULL;` returns 0
- [ ] SC-003 — §8 restore returned identical content + tags
- [ ] SC-004 — §9 scope checks both failed as expected
- [ ] SC-005 — §11 Grafana panel populated
- [ ] SC-006 — §6 `memory_search` returned in < 1s at the fixture scale
- [ ] SC-007 — total elapsed time from §1 to §6 < 15 min
- [ ] SC-008 — §12 rogue-agent drill passed clean
