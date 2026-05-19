# Contract: Write-Endpoint Response Shape

**Sprint**: 003-non-agentic-writes
**Endpoints**: `POST /memory/facts`, `POST /memory/events`, `POST /memory/knowledge/index`
**Change category**: additive (back-compat for existing Phase 1/2 clients)

This contract describes the `path` and `dissent_id` fields added to
each write-endpoint response in sprint 003. All other response
fields keep their Phase 1/2 definitions exactly.

## Added fields

| Field         | Type    | Always present? | Notes |
|---------------|---------|-----------------|-------|
| `path`        | string  | yes             | one of `"canonical"` or `"dissent"`. |
| `dissent_id`  | string  | only when `path == "dissent"` | UUID v4 of the row inserted into `dissents`. |

When `path == "canonical"`, `dissent_id` MUST be omitted (not
`null`). Clients that match on field-presence work correctly with
either convention; we pick "omitted" for symmetry with the Phase 2
dissent response.

## Example — `POST /memory/facts`, canonical path

Request (unchanged from Phase 2):

```http
POST /memory/facts HTTP/1.1
Authorization: Bearer <token>
Content-Type: application/json

{
  "type": "UserFact",
  "source": "User",
  "payload": { "name": "Ken", "host": "kubs0" }
}
```

Response (sprint-003 shape):

```http
HTTP/1.1 200 OK
Content-Type: application/json

{
  "id": "9c6f...",
  "version": 3,
  "path": "canonical"
}
```

## Example — `POST /memory/facts`, dissent path

Request:

```http
POST /memory/facts HTTP/1.1
Authorization: Bearer <token>
Content-Type: application/json

{
  "type": "UserFact",
  "source": "AgentProposal",
  "payload": { "name": "NotKen", "host": "kubs0" }
}
```

Response:

```http
HTTP/1.1 202 Accepted
Content-Type: application/json

{
  "path": "dissent",
  "dissent_id": "f4a2..."
}
```

The 202 status code is unchanged from Phase 2's dissent diversion
behavior; only the response body grows the explicit `path` field.

## Example — `POST /memory/events`

Events do not have a dissent path (the table is append-only). The
field is still returned for uniformity; it will always be
`"canonical"`.

```json
{ "id": "7b1f...", "path": "canonical" }
```

## Example — `POST /memory/knowledge/index`

Same as events — knowledge writes never divert; `path` always
`"canonical"`.

## Back-compat audit (clients we know of)

| Client | Read path | Status |
|--------|-----------|--------|
| `klams-client` Rust crate | `serde_json::from_value::<T>` with `#[serde(default)]` on optional fields, ignores unknown | unaffected (additive). |
| Viewport TypeScript client | `JSON.parse` then accessing known field names | unaffected (ignores unknown). |
| Phase 2 integration tests | match exact 200/202 status + presence of `dissent_id` | unaffected — `dissent_id` is still present on dissent path; new `path` field is ignored by these tests. |
| `curl` / ad-hoc | inspects raw JSON; tolerates additions | unaffected. |

## Contract tests

In `crates/klams-api/tests/contract_facts.rs` (existing file gains
two new rows):

| Test | What it asserts |
|------|-----------------|
| `facts_canonical_write_response_has_path_canonical_no_dissent_id` | 200 response carries `path: "canonical"` and omits `dissent_id`. |
| `facts_dissent_diversion_response_has_path_dissent_and_id` | 202 response carries `path: "dissent"` and a parseable UUID `dissent_id`. |

Equivalent rows added in `contract_events.rs` and
`contract_search.rs` (where applicable).

## Metric

`klams_writes_total{type, source, path}` is incremented exactly
once per write call, with `path` matching the response field.
Validated by `us3a_policy_endpoint.rs` integration test (which also
exercises the metrics scrape).

## Versioning / drift

This is an **additive** change. Removing or renaming `path` in a
future sprint would be a breaking change and MUST follow the
versioned-route convention in
[memory_policy.md](memory_policy.md#versioning--drift).
