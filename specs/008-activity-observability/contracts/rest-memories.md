# REST Endpoint — `GET /v1/memories` (sprint 008)

Cross-author, all-kinds memory listing. Backs the viewport's
`/activity` tab and is a generally useful operator surface.

**Mount**: `klams-api`, under the existing protected `/v1/*` router.
**Auth**: `Authorization: Bearer <token>` where the token's scope set includes `read`.
**Read or write**: read.
**Maintenance-window behavior**: reads served during maintenance (same as the rest of the REST API).

---

## Request

```
GET /v1/memories
    ?since=<RFC3339>                # default: now - 24h, inclusive
    &until=<RFC3339>                # default: now, exclusive
    &kinds=fact,knowledge,event     # default: all
    &state=live|deleted|all         # default: live
    &authors=<uuid>,<uuid>,...      # default: unrestricted
    &limit=<1..=200>                # default: 50
    &cursor=<opaque>                # default: omitted
```

**Validation order** (handler runs these before dispatching to the store):

1. Parse `since` / `until` as RFC3339 → `400 INVALID_TIMESTAMP` on parse failure (existing code).
2. Apply defaults for any omitted field.
3. `since <= until` → otherwise `400 INVALID_WINDOW`.
4. `(until - since) <= api.memories_max_window_days days` → otherwise `400 WINDOW_TOO_LARGE`.
5. Parse `kinds`, `state`, `authors` → `400 INVALID_FILTER` on a bad enum value or malformed UUID.
6. Clamp `limit` to `1..=200`.
7. Decode `cursor` → `400 INVALID_CURSOR` on malformed base64 (existing code).

---

## Response (200 OK)

```json
{
  "memories": [
    {
      "id": "01HZ8Q...",
      "kind": "fact",
      "content": { /* PublicMemoryContent variant */ },
      "tags": ["..."],
      "author": {
        "agent_name": "GHCP",
        "model": "claude-opus-4.7",
        "repo": "/home/ken/src/ai/klams"
      },
      "created_at": "2026-05-25T14:18:02Z",
      "updated_at": "2026-05-25T14:18:02Z",
      "state": "live"
    },
    {
      "id": "01HZ8R...",
      "kind": "knowledge",
      "content": { /* ... */ },
      "tags": [],
      "author": { /* ... */ },
      "created_at": "2026-05-25T13:55:00Z",
      "updated_at": "2026-05-25T13:55:00Z",
      "state": "deleted",
      "deleted_at": "2026-05-25T14:02:00Z",
      "deleted_by": { /* PublicAuthorRef */ }
    }
  ],
  "next_cursor": "Zjox..."   // null when window exhausted
}
```

**Per-row shape**:

- `id`, `kind`, `content`, `tags`, `author`, `created_at`, `updated_at` are the existing `PublicMemory` fields (sprint 007).
- `state` is always present, `"live"` or `"deleted"`.
- `deleted_at` and `deleted_by` are present **only** for `state = "deleted"` rows (per `serde(skip_serializing_if = "Option::is_none")`).
- `deleted_by` is a `PublicAuthorRef` matching the author who issued the soft-delete (looked up from `deleted_by_author_id` via a bulk fetch).

**Ordering**: newest-first by `(created_at, id)` across all kinds (R-005). Stable ties are broken by kind in the order `fact > knowledge > event`.

**Empty result**: `{"memories": [], "next_cursor": null}` — never an error.

---

## Errors

| Status | `error_code` | Meaning | Body |
|--------|--------------|---------|------|
| 400 | `INVALID_TIMESTAMP` | `since` or `until` not RFC3339 | existing handler error shape |
| 400 | `INVALID_WINDOW` | `since > until` | `{"error_code": "INVALID_WINDOW", "message": "..."}` |
| 400 | `WINDOW_TOO_LARGE` | `(until − since) > api.memories_max_window_days` | `{"error_code": "WINDOW_TOO_LARGE", "message": "...", "window_max_days": 30}` |
| 400 | `INVALID_FILTER` | unknown value in `kinds` / `state` or malformed UUID in `authors` | existing handler error shape |
| 400 | `INVALID_CURSOR` | cursor failed base64 / format decode | existing handler error shape |
| 401 | — | missing or invalid bearer token | existing |
| 403 | `INSUFFICIENT_SCOPE` | token does not hold `read` | existing handler error shape |

---

## Pagination

Cursor encoding is the existing base64-url-safe `section:ts_nanos:id`
shape from sprint 007 (R-003). The handler tracks per-section cursors
internally and surfaces a single opaque string to the client. Clients
MUST treat the cursor as opaque.

A response with fewer than `limit` rows and `next_cursor = null`
indicates the window is exhausted.

---

## Performance

Target p95 < 200 ms for the default 24h window over the homelab
corpus (≤ 10k facts + ≤ 50k knowledge items). Backed by:

- `idx_facts_created_at` and `idx_facts_author_id` (sprint 007).
- `idx_events_created_at` and `idx_events_author_id` (sprint 007).
- Qdrant payload index on `created_at` (sprint 005).

---

## Tests (drive implementation)

| FR | Test slot |
|----|-----------|
| FR-006 | `tests/integration/api_memories_list.rs::default_window_returns_all_kinds_all_authors` |
| FR-007 | `tests/integration/api_memories_list.rs::query_param_defaults_applied` |
| FR-007 | `tests/integration/api_memories_list.rs::kinds_filter_narrows_response` |
| FR-007 | `tests/integration/api_memories_list.rs::authors_filter_narrows_response` |
| FR-008 | `tests/integration/api_memories_list.rs::cursor_pagination_round_trip_newest_first` |
| FR-009 | `tests/integration/api_memories_window_cap.rs::window_too_large_returns_window_max_days` |
| FR-010 | `tests/integration/api_memories_deleted_state.rs::state_deleted_surfaces_deleted_at_and_by` |
| FR-010 | `tests/integration/api_memories_deleted_state.rs::state_live_omits_deletion_fields` |
| FR-011 | `tests/integration/api_memories_list.rs::no_embedding_pipeline_invoked` |
| edge   | `tests/integration/api_memories_list.rs::unknown_author_uuid_is_silently_ignored` |
| edge   | `tests/integration/api_memories_list.rs::inverted_window_returns_invalid_window` |
| edge   | `tests/integration/api_memories_list.rs::empty_window_returns_empty_list` |
