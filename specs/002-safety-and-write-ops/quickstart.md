# Quickstart: Safety, Drift Control, and the User View

This walkthrough validates every Story 1–5 happy path against a
running stack. It assumes the 001 quickstart is satisfied (Postgres,
Qdrant, TEI on `kubs0`, viewport build pipeline working). Where this
sprint changes the workflow, the differences are called out.

## Prerequisites

- Working repo clone on `kubs0` (or a dev machine that can reach
  `kubs0`'s exposed ports).
- Docker + Compose v2.
- Rust toolchain pinned by `rust-toolchain.toml`.
- **NEW**: [`just`](https://github.com/casey/just) installed:

  ```bash
  # Debian / Ubuntu
  curl -fsSL https://just.systems/install.sh | bash -s -- --to ~/.local/bin
  # or via cargo
  cargo install just
  ```

  Verify: `just --version` prints `>= 1.x`.

## 1. Bring the stack up (`just compose-up`)

```bash
cd ~/src/ai/klams
just compose-up
```

Equivalent to `docker compose -f deploy/docker-compose.yml up -d`.
Wait until Compose reports Postgres, Qdrant, and TEI as healthy.

Sanity:

```bash
just health
# curls /healthz and runs scripts/verify-mvp.sh --light
```

Both checks must report green within 30 seconds (SC-007).

## 2. Apply Phase 2 migration

Migrations apply automatically at service startup, but if you are
running an existing service you can re-run them explicitly:

```bash
cargo run -p klams-service -- migrate
```

The new `0002_dissents.sql` creates the `dissents` table, adds the
`dissent_count` column to `facts`, and installs the count-maintenance
triggers and the BEFORE-DELETE orphan trigger.

## 3. Run the service (`just run`)

```bash
just run
# = cargo run -p klams-service, logs to stderr
```

Leave it running in a foreground terminal. Open a second shell for
the curl steps below. (`docs/setup.md` documents the systemd
alternative; that switchover is deferred to sprint 003.)

Export your bearer token:

```bash
export KLAMS_TOKEN=$(grep bearer_token deploy/config/klams.toml | cut -d'"' -f2)
export KLAMS_URL=http://127.0.0.1:7777
H="-H \"Authorization: Bearer $KLAMS_TOKEN\""
```

## 4. Story 1 — Validation rejects bad agent writes

Send an agent-sourced `UserFact` missing the required `name` field:

```bash
curl -s -X POST "$KLAMS_URL/memory/facts" \
  -H "Authorization: Bearer $KLAMS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
        "type": "UserFact",
        "payload": {"email":"alice@example.com"},
        "source": "AgentProposal"
      }' | jq
```

Expected (`HTTP 422`):

```json
{
  "code": "validation_error",
  "message": "payload failed per-type validation",
  "details": [
    {"field": "payload.name", "rule": "required", "message": "field is required"}
  ]
}
```

Confirm nothing was written:

```bash
curl -s -H "Authorization: Bearer $KLAMS_TOKEN" \
  "$KLAMS_URL/memory/facts?type=UserFact" | jq '.items | length'
# → 0 (assuming a fresh DB; otherwise count is unchanged from the prior step)
```

Repeat with a bad hostname to trip the universal `hostname_shape`
sanity rule:

```bash
curl -s -X POST "$KLAMS_URL/memory/facts" \
  -H "Authorization: Bearer $KLAMS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"type":"TaskFact","payload":{"task_id":"3a7d…","status":"planned","hostname":"not a hostname!"},"source":"AgentProposal"}' | jq
```

→ `422 validation_error` with `details[0].rule = "hostname_shape"`.

## 5. Story 2 — Dissent on lower-trust contradiction; promote later

Write a `User`-sourced fact first:

```bash
curl -s -X POST "$KLAMS_URL/memory/facts" \
  -H "Authorization: Bearer $KLAMS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"type":"UserFact","payload":{"name":"Ken","city":"Brisbane"},"source":"User"}' | jq
# → 200 Persisted; record .id as FACT_ID and .version as V
export FACT_ID=…
```

Submit a contradicting `AgentProposal` to the same `(type, payload key)`:

```bash
curl -s -X POST "$KLAMS_URL/memory/facts" \
  -H "Authorization: Bearer $KLAMS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d "{\"type\":\"UserFact\",\"payload\":{\"name\":\"Ken\",\"city\":\"Sydney\"},\"source\":\"AgentProposal\",\"explicit_id\":\"$FACT_ID\",\"expected_version\":1}" | jq
```

Expected (`HTTP 202`):

```json
{
  "dissent_id": "…",
  "fact_id":    "…",
  "status":     "pending",
  "deduped":    false
}
```

The canonical fact is unchanged but its `dissent_count` is now 1:

```bash
curl -s -H "Authorization: Bearer $KLAMS_TOKEN" "$KLAMS_URL/memory/facts/$FACT_ID" | jq '.dissent_count'
# → 1
```

List the dissent:

```bash
curl -s -H "Authorization: Bearer $KLAMS_TOKEN" \
  "$KLAMS_URL/memory/dissents?fact_id=$FACT_ID" | jq '.items[0]'
```

Promote it (as `Controller` or `User`; the request body's `source`
must be one of those):

```bash
export DISSENT_ID=…
curl -s -X POST "$KLAMS_URL/memory/dissents/$DISSENT_ID/promote" \
  -H "Authorization: Bearer $KLAMS_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"source":"User","expected_version":1}' | jq
```

Expected: `200 OK` with the updated `Fact` (version=2, source="User",
payload.city="Sydney", dissent_count=0).

Stale-version retry path (Story 2 Acceptance Scenario 5 / SC-003):

```bash
# Two writers race against version=2
curl … -d '{"...","expected_version":2}'  # first wins
curl … -d '{"...","expected_version":2}'  # second:
# → 409 version_conflict, body has "current_version": 3
```

## 6. Story 3 — Decay-aware ranking

Configure two facts with equivalent text relevance and different
types. Then force a decay pass (the task runs on `task_interval_seconds`
by default, but the service exposes a debug endpoint **only in test
builds**; in production wait for the scheduled run):

```bash
just test -- --test us3_decay
```

The `us3_decay` integration test seeds the two facts, advances
simulated time, runs one decay batch, and asserts:

- the `TaskFact`'s `decay_weight` drops more than the `UserFact`'s,
- `POST /memory/search` returns the `UserFact` ahead of the
  `TaskFact` on a query that matches both.

## 7. Story 4 — Viewport curation flow

Cross-build and run the viewport (instructions per 001 + the new
`just viewport-build`):

```bash
just viewport-build
# resulting klams-viewport.exe under viewport/src-tauri/target/x86_64-pc-windows-msvc/release/
```

Run it on Windows (or via Wine for smoke), point it at
`http://kubs0:7777` with your bearer token, then:

1. Open the **Facts** page. Click the row created in §5. The
   **Provenance** panel shows `source=User`, `version=2`,
   `created_at`, `updated_at`, `last_used_at`, `decay_weight`,
   `confidence`, `dissent_count=0`.
2. Click **Edit**. Change `city` to `Melbourne`. Confirm. The
   viewport applies the change optimistically and the next refresh
   confirms `version=3`.
3. Trigger **Delete**, confirm the dialog. The row vanishes;
   refreshing the list confirms it is gone.
4. Open the **Dissents** page. Submit a fresh dissent via curl
   (§5 redux), reload the page; the new dissent appears with the
   diff against canonical. Click **Discard**; the row drops to
   `status=discarded` and disappears from the default view.

## 8. Story 5 — `just gate` is the constitution gate

```bash
just gate
```

Equivalent to:

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

Any failure causes `just gate` to exit non-zero. CI runs exactly this
command (no separate CI shim).

Tear the stack down:

```bash
just compose-down
```

## Pre-commit gate (reminder)

Per the [klams constitution §"Pre-Commit Checks"](../../.specify/memory/constitution.md#pre-commit-checks),
every commit MUST pass `just gate`. The CI workflow at
`.github/workflows/ci.yml` invokes the same command; no developer
machine should diverge from CI on what the gate checks.
