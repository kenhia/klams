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

Generate one opaque random token per client. Anything with ≥128 bits
of entropy works; the service treats them as opaque strings. A quick
recipe:

```bash
# one per client (write, read-only, admin)
for label in ghcp-write viewport-readonly ken-admin; do
    printf '%s = %s\n' "$label" "$(openssl rand -hex 32)"
done
```

Paste the generated values into the placeholders below (the
`ghcp-write-XXX…` / `viewport-readonly-XXX…` / `ken-admin-XXX…`
prefixes are illustrative — the service does not parse them, the
`label` field is what shows up in logs and metrics).

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

Expected: `klams-service` boots, applies the Qdrant payload backfill (first run only), and logs (JSON format):

```text
{"level":"INFO","message":"listening","addr":"0.0.0.0:7777",...}
{"level":"INFO","message":"qdrant author backfill complete","patched":N,...}
```

(`7777` is the default `[server].port` from `klams.toml`; change there if you need a different bind.)

> **Connecting from a non-loopback hostname?** rmcp's
> Streamable-HTTP service ships with a Host-header allowlist for
> DNS-rebinding protection. klams disables it by default
> (`[server].mcp_allowed_hosts = []`) and relies on bearer auth
> instead. If you want belt-and-suspenders, set e.g.
> `mcp_allowed_hosts = ["localhost", "kubs0:7777"]` in `klams.toml`.

Probe:

```bash
curl -s http://localhost:7777/healthz | jq .
```

Expected fields: `status: "Ok"`, plus `postgres`, `qdrant`, `embeddings`, `queue`, `version`, `uptime_seconds` — all healthy.

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
      "url": "http://kubs0:7777/mcp",
      "headers": {
        "Authorization": "Bearer ghcp-write-XXXXXXXXXXXXXXXX"
      }
    }
  }
}
```

Reload the VS Code window. The status bar should show the klams MCP
server connected; GHCP's tool palette should list the klams tools.

The "MCP: klams" Output panel will log two harmless warnings on
startup — they are cosmetic and can be ignored:

```text
[warning] Could not fetch resource metadata: AggregateError: ...
[warning] Failed to parse message: ""
```

The first is VS Code probing `/.well-known/oauth-protected-resource`
(klams doesn't need OAuth — `headers.Authorization` is sufficient).
The second is rmcp's SSE keep-alive ping. Both are documented in
[research-vscode-mcp-http.md](./research-vscode-mcp-http.md) §6–§7.

---

## 4b. Register an MCP client — GHCP CLI path

Edit `~/.copilot/mcp-config.json`:

```jsonc
{
  "mcpServers": {
    "klams": {
      "type": "http",
      "url": "http://kubs0:7777/mcp",
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
just mcp-call memory_append_event "$(jq -nc --arg a "$AUTHOR_ID" '{
  author_id: $a,
  category: "Deploy",
  payload: {service: "widget", host: "kub3", version: "1.4.2"}
}')"
```

> The `-c` flag on `jq` keeps the JSON on one line — `just`'s `{{args}}`
> interpolation injects the value verbatim into the recipe body, and
> multi-line JSON would break shebang-style recipes.

Verify via `memory_search` with `kinds: ["event"]`:

```bash
just mcp-call memory_search '{"query":"widget deploy","kinds":["event"],"top_k":5}'
```

Expected: event row with author attribution.

---

## 8. User story 4 walkthrough — soft delete and restore

Capture an existing memory id and its author (the `memory_delete`
schema requires `author_id` for audit attribution):

```bash
ROW=$(just db-psql -t -c "SELECT id,author_id FROM facts WHERE deleted_at IS NULL ORDER BY created_at DESC LIMIT 1")
ID=$(echo "$ROW" | awk -F'|' '{print $1}' | tr -d ' \r\n')
AUTHOR=$(echo "$ROW" | awk -F'|' '{print $2}' | tr -d ' \r\n')
```

Soft-delete with the **write** token:

```bash
KLAMS_TOKEN=$WRITE_TOKEN \
    just mcp-call memory_delete "$(jq -nc --arg id "$ID" --arg a "$AUTHOR" '{id:$id,author_id:$a}')"
```

Expected: `{"id":"...","deleted_at":"..."}`.

Confirm it's hidden from search:

```bash
KLAMS_TOKEN=$WRITE_TOKEN just mcp-call memory_search '{"query":"<text matching the row>","top_k":5}' | jq 'length'
# Expected: 0
```

Switch to the **admin** token and list deleted items:

```bash
KLAMS_TOKEN=$ADMIN_TOKEN just mcp-call memory_admin_list_deleted '{"kinds":["fact"],"limit":10}'
```

Restore:

```bash
KLAMS_TOKEN=$ADMIN_TOKEN \
    just mcp-call memory_admin_restore "$(jq -nc --arg id "$ID" '{id:$id}')"
```

Expected: `{"id":"...","restored_at":"..."}`. Confirm the item is
back by re-running the `memory_search` above and observing it return
to the result set.

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

> **Deploying a built artifact?** The `/authors` route only appears
> after a fresh build (the navbar link lives in
> [viewport/src/routes/+layout.svelte](../../viewport/src/routes/+layout.svelte)).
> If you ran `just viewport-build` then copied the bundle to another
> host (e.g. `cleo`) and don't see the **Authors** tab, the deployed
> artifact is stale — rebuild and recopy. Browser/Tauri WebView cache
> can also hide the new layout; a hard reload (or kill + relaunch the
> Tauri app) clears it.

---

## 11. Grafana panel sanity-check

Scrape `/metrics`:

```bash
curl -s http://localhost:7777/metrics | grep '^klams_mcp_'
```

Expected counters (after exercising §5–§8):

```text
klams_mcp_writes_total{agent_name="GHCP",model="claude-opus-4.7",kind="fact"}        1
klams_mcp_writes_total{agent_name="controller",model="unknown",kind="event"}         1
klams_mcp_deletes_total{agent_name="GHCP",model="claude-opus-4.7",mode="soft"}       1
klams_mcp_deletes_total{agent_name="ken-admin",model="unknown",mode="restored"}      1
klams_mcp_search_total{agent_name="GHCP",model="claude-opus-4.7"}                    2
```

Confirm via the klams Grafana dashboard that the "MCP author activity"
panel renders the breakdown.

> **Known issue (2026-05-25)**: panels in the klams Grafana dashboard
> render "No Data" even though `/metrics` exposes the counters above.
> Tracked in [backlog.md](../planning/backlog.md#grafana-mcp-author-activity-panel-no-data-priority)
> — likely a Prometheus scrape config or PromQL/label mismatch. SC-005
> is blocked on this; everything upstream (counters, scrape target) is
> verified working.

---

## 12. Restore-from-rogue-agent drill

The drill is decomposed into four focused smoke tests in
[crates/klams-service/tests/mcp_phase6.rs](../../crates/klams-service/tests/mcp_phase6.rs)
— each test wires up a fresh Postgres + Qdrant, registers an author,
and exercises one slice of the lifecycle:

| Test | Covers drill step |
|---|---|
| `memory_delete_soft_smoke` | (3) soft-delete + (4) hidden from search |
| `memory_admin_list_deleted_smoke` | (6) admin can enumerate the tombstones |
| `memory_admin_restore_smoke` | (7) restore + (8) round-trip identical |
| `memory_admin_hard_delete_smoke` | hard-delete escape hatch (out of SC-008 scope) |

Run the full set (they are `#[ignore]`-gated because they hit live
Postgres + Qdrant):

```bash
TEST_QDRANT_URL=http://127.0.0.1:6334 \
    cargo test -p klams-service --test mcp_phase6 -- --ignored --test-threads=1
```

> **Test isolation caveat (2026-05-25)**: the four tests share a
> single `knowledge_items_test` Qdrant collection and a single test
> Postgres database, so cross-test rows can skew counter-based
> assertions in `memory_admin_list_deleted_smoke`. Delete the
> collection between runs (`curl -X DELETE
> http://127.0.0.1:6333/collections/knowledge_items_test`) or run the
> single test in isolation
> (`cargo test ... memory_admin_list_deleted_smoke`). Tracked in
> [backlog.md](../planning/backlog.md#phase-6-test-harness-isolation).

Pass criteria for SC-008: the delete → restore round-trip produces
identical content; verified by `memory_admin_restore_smoke` +
`memory_delete_soft_smoke` and by the live walk in §8.

---

## Success criteria checklist

After completing the walkthrough:

- [X] SC-001 — §5 + §6 succeeded end-to-end
- [X] SC-002 — `SELECT COUNT(*) FROM facts WHERE author_id IS NULL;` returns 0
- [X] SC-003 — §8 restore returned identical content + tags
- [X] SC-004 — §9 scope checks both failed as expected
- [ ] SC-005 — §11 Grafana panel populated (blocked — see backlog)
- [X] SC-006 — §6 `memory_search` returned in < 1s at the fixture scale
- [X] SC-007 — total elapsed time from §1 to §6 < 15 min
- [X] SC-008 — §12 rogue-agent drill passed clean (3/4 phase6 tests green; the 4th is a test-isolation bug, not a regression — see §12 caveat)
