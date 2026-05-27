# MCP Tool — `event_search` (sprint 008)

Read-only MCP tool that returns events filtered by author, category,
date range, and exact-equality JSON payload match. Pure SQL — no
embedding pipeline involvement.

**Scope required**: `read` (visible in `tools/list` for any token holding `read`).

**Read or write**: read.

**Maintenance-window behavior**: not gated (reads continue during maintenance windows, same as sprint 007 read tools).

---

## Input

JSON Schema: [tool-schemas/event_search.json](./tool-schemas/event_search.json).

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `author_id` | UUID string \| array of UUID strings | unrestricted | Single ID or list; empty list is the same as omitted. |
| `category` | string \| array of strings | unrestricted | Single category or list; empty list is the same as omitted. |
| `since` | RFC3339 timestamp | `now − 24h` | Inclusive. |
| `until` | RFC3339 timestamp | `now` | Exclusive. |
| `payload_match` | object | omitted | Each `key: value` pair must match exactly via JSONB containment (R-010). Nested objects match by containment; arrays match by sub-array containment. |
| `limit` | integer | 50 | Range `1..=500`. |
| `order` | `"desc"` \| `"asc"` | `"desc"` | Ordering on `(created_at, id)`. |
| `cursor` | opaque string | omitted | Returned by a previous call. |

**Window validation** (R-002):

- If `since > until` → MCP error envelope with `_meta.error_code = "INVALID_WINDOW"`.
- If `(until − since) > api.memories_max_window_days` (default 30) → MCP error envelope with `_meta.error_code = "WINDOW_TOO_LARGE"` and `_meta.window_max_days` populated.

---

## Output (success)

Standard MCP `content` array carrying a single JSON document:

```json
{
  "events": [
    {
      "id": "01HZ8Q...",
      "kind": "event",
      "content": {
        "event": {
          "category": "Deploy",
          "payload": { "service": "widget", "version": "1.2.3" },
          "task_id": null
        }
      },
      "tags": [],
      "author": {
        "agent_name": "controller",
        "model": null,
        "repo": "/home/ken/src/ops/widget"
      },
      "created_at": "2026-05-25T14:18:02Z",
      "updated_at": "2026-05-25T14:18:02Z"
    }
  ],
  "next_cursor": "ZTo..."   // null when exhausted
}
```

**Projection**: each event is a `PublicMemory` with `kind: "event"` —
the same shape returned by `memory_search` and `GET /v1/authors/{id}/memories`.
Soft-delete metadata is **not** present (events are append-only and
have no deletion state per sprint 007 R-006).

**Empty result**: `{"events": [], "next_cursor": null}` — never an error.

---

## Output (error)

Standard MCP error envelope (sprint 007):

```json
{
  "isError": true,
  "content": [{ "type": "text", "text": "..." }],
  "_meta": {
    "error_code": "WINDOW_TOO_LARGE",
    "window_max_days": 30
  }
}
```

Possible `_meta.error_code` values:

| Code | When |
|------|------|
| `INVALID_WINDOW` | `since > until` |
| `WINDOW_TOO_LARGE` | `until − since > api.memories_max_window_days` |
| `INSUFFICIENT_SCOPE` | Token lacks `read` (filtered out of `tools/list` already; here for completeness) |
| `INTERNAL_ERROR` | Unexpected store-layer failure |

See [error-codes.md](./error-codes.md) for the canonical table.

---

## Pagination

Cursor encoding is the existing base64-url-safe `section:ts_nanos:id`
shape from sprint 007 (`section = "e"` for event_search). Clients
MUST treat the cursor as opaque.

A response with fewer than `limit` rows and `next_cursor = null`
indicates the window is exhausted.

---

## Tests (drive implementation)

| FR | Test slot |
|----|-----------|
| FR-001 | `tests/integration/mcp_event_search.rs::filter_by_category_and_window` |
| FR-001 | `tests/integration/mcp_event_search.rs::filter_by_payload_match` |
| FR-002 | `tests/integration/mcp_event_search.rs::cursor_pagination_round_trip` |
| FR-003 | `tests/integration/mcp_event_search.rs::visible_with_read_scope_only` |
| FR-004 | `tests/integration/mcp_event_search.rs::no_embedding_pipeline_invoked` |
| FR-005 | `tests/integration/mcp_event_search.rs::projection_carries_author_subset` |
| FR-009 | `tests/integration/mcp_event_search_window.rs::window_too_large_returns_meta_max` |
| edge   | `tests/integration/mcp_event_search_window.rs::inverted_window_returns_invalid_window` |
| edge   | `tests/integration/mcp_event_search_window.rs::empty_window_returns_empty_list` |
| edge   | `tests/integration/mcp_event_search.rs::empty_payload_match_matches_all_in_window` |

### Edge case: empty `payload_match`

When `payload_match: {}` is supplied (an empty object), the JSONB containment predicate `'{}'::jsonb @> payload` is **always true**, so the filter degenerates to "all events in the date window". This is intentional and matches PostgreSQL JSONB semantics. Callers that want "no filter" SHOULD omit the field; callers that pass `{}` will see equivalent behavior. Documented here so the test `empty_payload_match_matches_all_in_window` codifies the contract rather than the implementation accidentally drifting.
