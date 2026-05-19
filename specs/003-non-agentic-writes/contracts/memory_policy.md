# Contract: `GET /memory/policy`

**Sprint**: 003-non-agentic-writes
**Endpoint**: `GET /memory/policy`
**Auth**: existing bearer token (same middleware as other endpoints)
**Idempotent**: yes (pure read; no side effects)

## Request

```http
GET /memory/policy HTTP/1.1
Authorization: Bearer <token>
```

No query parameters. No request body.

## Response — 200 OK

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "User": {
    "rank": 4,
    "description": "Direct user input via viewport or CLI; wins all contradictions."
  },
  "Controller": {
    "rank": 3,
    "description": "Controller process on a trusted homelab machine; wins over Task and below."
  },
  "Task": {
    "rank": 2,
    "description": "Ansible plays, scanner, monitors, controller execution traces."
  },
  "AgentProposal": {
    "rank": 1,
    "description": "Agent-originated writes; diverted to dissents when they contradict any higher-trust row."
  }
}
```

### Field semantics

- `rank` (integer, 1..=255): the integer the dispatcher compares.
  Higher = more trusted. Two sources with the same rank are not
  produced by this sprint; values are spaced to leave room.
- `description` (string): free-form human-readable explanation. Not
  parsed by clients.
- Keys are exactly the four `MemorySource` enum variants. Order is
  not significant; clients MUST key by name.

## Response — 401 Unauthorized

Bearer token missing or invalid. Existing 401 response shape, no
new fields.

## Response — 5xx

Implementation note: the handler serializes an `Arc<PolicyTable>`
held in app state. There is no I/O. A 5xx from this endpoint is
**always** a bug in klams itself, never a downstream failure.

## Contract tests

The following tests gate this contract. All live in
`crates/klams-api/tests/contract_policy.rs` and are required to
land **before** the handler (TDD per the constitution).

| Test | What it asserts |
|------|-----------------|
| `policy_endpoint_returns_all_four_sources` | Response body is a JSON object with exactly the four keys `User`, `Controller`, `Task`, `AgentProposal`. |
| `policy_endpoint_ranks_are_strictly_descending` | `User.rank > Controller.rank > Task.rank > AgentProposal.rank`. |
| `policy_endpoint_requires_bearer` | Missing/invalid bearer returns 401 with the existing error shape. |
| `policy_endpoint_matches_dispatcher` | The JSON response, when round-tripped back through `serde_json::from_str`, equals the `PolicyTable` value the dispatcher holds. This is the no-drift guarantee for FR-018 / SC-005. |

## Observability

- One Prometheus counter `klams_http_requests_total{path="/memory/policy"}` (existing per-route counter, no new metric).
- One `tracing::info!` span on each request (existing middleware).

## Versioning / drift

The response shape above is pinned to klams sprint-003. A future
breaking change MUST:

1. Add a new versioned route (`GET /memory/policy/v2`), **not** alter
   the v1 response.
2. Update `docs/usage.md` and the handoff document's "drift" section
   with the deprecation timeline.
