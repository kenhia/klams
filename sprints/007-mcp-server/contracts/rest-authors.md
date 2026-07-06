# REST Endpoints — Author drilldown (sprint 007)

Three new endpoints are added under the existing `klams-api`
protected mount. All require the `read` scope (FR-024a). They back
the viewport's `/authors` route (R-008) and stay on the REST contract
so the viewport remains MCP-unaware.

Base URL: same as existing API (e.g., `http://kubs0:8088`).

Auth: `Authorization: Bearer <token>` where the token's scope set
includes `read`.

---

## `GET /v1/authors`

List registered authors with rolled-up activity counts.

**Query params**:

| Param  | Type      | Default | Notes |
|--------|-----------|---------|-------|
| `limit` | integer  | 50      | Max 200 |
| `cursor` | string  | null    | Opaque pagination cursor returned by previous call |
| `agent_name` | string | null  | Optional exact-match filter |
| `since` | ISO-8601 | null    | `last_seen_at >= since` |

**Response** (200):

```json
{
  "authors": [
    {
      "id": "01HZ8Q...",
      "agent_name": "GHCP",
      "model": "claude-opus-4.7",
      "session_title": "Phase 7 design",
      "repo": "/home/ken/src/ai/klams",
      "client_app": "VS Code",
      "client_version": "1.118.0",
      "created_at": "2026-05-24T17:00:00Z",
      "last_seen_at": "2026-05-24T18:42:11Z",
      "counts": {
        "writes": 42,
        "soft_deletes": 1,
        "restores_received": 1,
        "events": 7
      }
    }
  ],
  "next_cursor": "..." // null when exhausted
}
```

Errors:

- `401 Unauthorized` — missing/invalid bearer
- `403 Forbidden` — token lacks `read` scope
- `400 Bad Request` — invalid `limit`, malformed `cursor`, malformed `since`

---

## `GET /v1/authors/{id}`

Fetch one author by id.

**Path params**: `id` — UUID.

**Response** (200): same author object as in the list, including `counts`.

Errors: `401`, `403`, `404 Not Found`.

---

## `GET /v1/authors/{id}/memories`

List memory items written by this author.

**Path params**: `id` — UUID.

**Query params**:

| Param  | Type      | Default | Notes |
|--------|-----------|---------|-------|
| `limit` | integer  | 50      | Max 200 |
| `cursor` | string  | null    | Opaque pagination cursor |
| `kinds`  | comma-list | null  | Subset of `fact,knowledge,event`; default = all |
| `state`  | enum     | `live`  | `live \| deleted \| all` |

**Response** (200):

```json
{
  "memories": [
    {
      "id": "01HZ8Q...",
      "kind": "fact",
      "content": { /* PublicMemoryContent */ },
      "tags": [...],
      "author": { /* PublicAuthorRef */ },
      "created_at": "...",
      "updated_at": "...",
      "state": "live",                          // live | deleted
      "deleted_at": null,                        // populated when state = deleted
      "deleted_by": null                         // PublicAuthorRef when state = deleted
    }
  ],
  "next_cursor": null
}
```

Errors: `401`, `403`, `404 Not Found` (author), `400 Bad Request` (bad
`state`/`kinds`/`limit`/`cursor`).

---

## Notes

- All endpoints honor the sprint-006 maintenance window for **reads**: maintenance does NOT block reads (consistent with the REST API and FR-021).
- Pagination cursors are opaque strings encoding `(last_seen_id, last_seen_at)` tuples; clients MUST NOT parse them.
- Response shape mirrors existing list endpoints (`/v1/facts`, `/v1/knowledge`) for viewport consistency.
- `counts` aggregations come from indexed SQL queries (no N+1); benchmark target: < 50 ms for `/v1/authors?limit=50`.
