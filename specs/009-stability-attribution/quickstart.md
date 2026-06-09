# Quickstart — Sprint 009

Operator walkthrough for validating Stability & Attribution end-to-end
on a representative environment (`kubs0` style: Linux server, klams +
Postgres + Qdrant colocated).

## 1. Configure agent_name on a token

Edit `klams.toml`:

```toml
[[auth.tokens]]
token = "<existing-bearer>"
scopes = ["read", "write"]
label = "alice's laptop"      # free-form human tag (logs / metrics)
agent_name = "alice"          # binds REST writes to author 'alice'
```

Restart klams-service. Confirm the startup log:

```text
INFO klams_service: token bound to author
  author_id=... agent_name=alice
```

### What gets attributed where

`agent_name` only governs the **REST** write endpoints (`POST
/memory/facts`, `/memory/events`, `/memory/knowledge`) — anything
authenticated by this bearer is stamped with this token's resolved
`author_id` instead of falling through to the seeded `system`
author. A token without `agent_name` keeps the back-compat
behaviour and writes as `system`.

The **MCP** tools (`memory_add`, `memory_append_event`, etc.) are
unchanged: callers there pass an explicit `author_id` they obtained
from `register_author`, and that identity is what's stored. So an
agent like GHCP that registers itself per session continues to be
attributed to its own registered author, not to whatever
`agent_name` happens to be bound to the bearer it's tunneling
through.

In short:

| Write path                                  | Author used                              |
|---------------------------------------------|------------------------------------------|
| REST `POST /memory/{facts,events,knowledge}` | Token's `agent_name` (or `system`)       |
| MCP `memory_add` / `memory_append_event`    | The `author_id` from `register_author`   |
| Bench seeder (REST, dedicated token)        | `klams-bench` (own bound token)          |

## 2. Verify per-author attribution on REST writes

```bash
curl -sS -i \
  -H "Authorization: Bearer <alice's token>" \
  -H 'Content-Type: application/json' \
  -d '{
    "type": "UserFact",
    "payload": {"name": "alice-quickstart", "value": "hello from alice"},
    "source": "AgentProposal"
  }' \
  http://kubs0:7777/memory/facts
```

(Adjust host / port to match your deployment — klams binds
`0.0.0.0:7777` by default, plain HTTP. `-i` prints the response
status line so a 4xx body doesn't look like a silent success.
`type` must be one of `UserFact`, `EnvFact`, `TaskFact`; `source`
is typically `AgentProposal`.)

In the viewport Activity tab, the new fact's row should display
`alice` (not `system`) as the author.

## 3. Run the half-close soak harness

For a quick smoke during the day:

```bash
just soak --duration 10m
```

For the SC-001 acceptance run (overnight, off-hours so it doesn't
interfere with daytime klams use):

```bash
just soak --duration 18h
```

Observe via `ss -tn | grep CLOSE_WAIT | wc -l` that the count
stabilizes well below the fd ceiling. SC-001 passes when end-of-run
fd and `CLOSE_WAIT` counts are at or below the start-of-run counts.

## 4. One-shot re-attribution repair

Always start with a dry run:

```bash
KLAMS_DATABASE_URL="postgres://klams:<password>@127.0.0.1:5432/klams" \
  cargo run --release -p klams-reattribute-system -- --dry-run \
  --report-out /tmp/reattribute-dryrun.json
```

`KLAMS_DATABASE_URL` defaults to `postgres://klams:klams@127.0.0.1:5432/klams`,
which won't match a real deployment — pull the live password from
`deploy/compose.env` (or `docker inspect klams-postgres`). The tool
also honors `KLAMS_QDRANT_URL` (default `http://127.0.0.1:6334`) and
`KLAMS_QDRANT_COLLECTION` (default `klams_knowledge`).

Inspect the report. Per-author counts should look plausible for the
deployment's recent activity. The `lost-author` bucket collects
rows whose true writer couldn't be unambiguously recovered (no
provenance, ambiguous provenance, or recovered author no longer
exists). When satisfied:

```bash
KLAMS_DATABASE_URL="postgres://klams:<password>@127.0.0.1:5432/klams" \
  cargo run --release -p klams-reattribute-system -- --apply \
  --report-out /tmp/reattribute-apply.json
```

Re-run dry-run to confirm idempotency (both reassignment counters
at 0).

## 5. Refresh the perf baseline

With #26 closed and attribution wired:

```bash
just bench-clean
just bench-seed
just bench-run --samples 100 --queries 10
```

Compare against `specs/008-activity-observability/perf-baseline.md`;
commit the refreshed file if numbers shift materially.

## 6. Validate the Authors view fix (kwi #28)

Open the viewport. Navigate Authors → click a memory summary row.
The details pane should open in-place (not a 404). Verify for one
fact, one event, and one knowledge item.

## 7. Phase 6 test isolation

```bash
cargo test --workspace
```

Confirm the suite passes without `--test-threads=1` and without
`--ignored`. Re-run 10 times in succession — all must pass.

## Operational notes

- The `[service.limits]` TOML section is optional. Defaults
  (`header_read_timeout=30s`, `keep_alive_timeout=75s`,
  `per_peer_max_concurrent=64`) cover normal deployments.
- The systemd unit now sets `LimitNOFILE=65536`. Reload
  (`systemctl daemon-reload && systemctl restart klams`) after
  picking up the new unit file.
- `just bench-clean` is now author-based: it deletes everything
  attributed to the `klams-bench` author. There is no payload-pattern
  fallback.
