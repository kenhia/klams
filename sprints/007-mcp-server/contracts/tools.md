# MCP Tools — sprint 007

All tools follow MCP `tools/call` conventions: input is a single JSON
object whose schema is published in [`tool-schemas/`](./tool-schemas/);
output is an MCP `Result` with `content` (textual summary) and `_meta`
(machine-readable payload).

Tool scopes (FR-020):

| Tool                          | Required scope | Reference |
|-------------------------------|----------------|-----------|
| `register_author`             | `write`        | FR-004    |
| `memory_add`                  | `write`        | FR-009    |
| `memory_append_event`         | `write`        | FR-010    |
| `memory_search`               | `read`         | FR-011    |
| `memory_related`              | `read`         | FR-012    |
| `memory_delete`               | `write`        | FR-013    |
| `memory_admin_restore`        | `admin`        | FR-016    |
| `memory_admin_hard_delete`    | `admin`        | FR-016    |
| `memory_admin_list_deleted`   | `admin`        | FR-016    |

---

## `register_author`

**Input**: see [`tool-schemas/register_author.json`](./tool-schemas/register_author.json).

```json
{
  "agent_name": "GHCP",
  "model": "claude-opus-4.7",
  "session_title": "Phase 7 design",
  "repo": "/home/ken/src/ai/klams",
  "client_app": "VS Code",
  "client_version": "1.118.0",
  "extra": {}
}
```

**Output `_meta`**:

```json
{ "author_id": "01HZ8Q..." }
```

Errors: `INVALID_AGENT_NAME`, `INVALID_REPO_PATH`, `EXTRA_TOO_LARGE`.

---

## `memory_add`

**Input**: see [`tool-schemas/memory_add.json`](./tool-schemas/memory_add.json).

```json
{
  "author_id": "01HZ8Q...",
  "kind": "fact",                       // or "knowledge"
  "content": { /* per-kind payload */ },
  "tags": ["preferences", "tooling"]
}
```

Per-kind `content`:

- `kind = "fact"` — `{ "type": "UserFact" | "TaskFact" | "EnvFact" | ..., "payload": {...} }`. The `type` selects the validator on the server side.
- `kind = "knowledge"` — `{ "text": "...", "source_path": "?", "repo": "?" }`. Embedding is computed server-side; clients MUST NOT supply vectors (FR-009, R-012).

**Output `_meta`**: full `PublicMemory` of the resulting row (newly created or deduped).

Errors: `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `INVALID_KIND`,
`SCHEMA_VALIDATION_FAILED`, `EMBEDDING_UNAVAILABLE` (retryable),
`MAINTENANCE_WINDOW_ACTIVE`.

---

## `memory_append_event`

**Input**: see [`tool-schemas/memory_append_event.json`](./tool-schemas/memory_append_event.json).

```json
{
  "author_id": "01HZ8Q...",
  "category": "Deploy",
  "payload": { "service": "widget", "host": "kub3", "version": "1.4.2" },
  "task_id": null
}
```

**Output `_meta`**: `PublicMemory` of the appended event.

Errors: `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`,
`INVALID_CATEGORY` (must be non-empty), `MAINTENANCE_WINDOW_ACTIVE`.

---

## `memory_search`

**Input**: see [`tool-schemas/memory_search.json`](./tool-schemas/memory_search.json).

```json
{
  "query": "free-text natural language",
  "kinds": ["fact", "knowledge", "event"],     // optional; default = all
  "tags": ["preferences"],                       // optional; AND-of-tags
  "top_k": 10,                                   // optional; default 10, max 50
  "filters": { /* reserved for future use */ }
}
```

**Output `_meta`**:

```json
{ "results": [ /* PublicMemory[], soft-deleted excluded */ ] }
```

Errors: `INVALID_TOP_K` (> 50), `EMPTY_QUERY`.

---

## `memory_related`

**Input**: see [`tool-schemas/memory_related.json`](./tool-schemas/memory_related.json).

```json
{ "id": "01HZ8Q...", "top_k": 5 }
```

**Output `_meta`**: `{ "results": [PublicMemory[]] }` — semantic neighbors of the referenced item, excluding the item itself and soft-deleted rows.

Errors: `NOT_FOUND`, `INVALID_TOP_K`.

---

## `memory_delete`

**Input**: see [`tool-schemas/memory_delete.json`](./tool-schemas/memory_delete.json).

```json
{ "author_id": "01HZ8Q...", "id": "01HZ8Q..." }
```

**Behavior**: soft delete. Idempotent (second call on already-deleted row returns success without modifying `deleted_at` / `deleted_by_author_id`).

**Output `_meta`**: `{ "id": "...", "deleted_at": "<ISO-8601 UTC>" }`.

Errors: `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `NOT_FOUND`,
`EVENTS_NOT_DELETABLE` (FR-015), `MAINTENANCE_WINDOW_ACTIVE`.

---

## `memory_admin_restore`

**Input**: see [`tool-schemas/memory_admin_restore.json`](./tool-schemas/memory_admin_restore.json).

```json
{ "id": "01HZ8Q..." }
```

**Behavior**: clears `deleted_at` and `deleted_by_author_id`. The original deletion attribution is **not** recorded post-restore (it lives in audit logs only).

**Output `_meta`**: `{ "id": "...", "restored_at": "<ISO-8601 UTC>" }`.

Errors: `NOT_FOUND`, `NOT_SOFT_DELETED`, `INSUFFICIENT_SCOPE`.

---

## `memory_admin_hard_delete`

**Input**: see [`tool-schemas/memory_admin_hard_delete.json`](./tool-schemas/memory_admin_hard_delete.json).

```json
{ "id": "01HZ8Q..." }
```

**Behavior**: removes the row from Postgres (facts) or the point from Qdrant (knowledge). No effect on events (events aren't deletable; tool returns `EVENTS_NOT_DELETABLE`).

**Output `_meta`**: `{ "id": "...", "hard_deleted_at": "<ISO-8601 UTC>" }`.

Errors: `NOT_FOUND`, `EVENTS_NOT_DELETABLE`, `INSUFFICIENT_SCOPE`.

---

## `memory_admin_list_deleted`

**Input**: see [`tool-schemas/memory_admin_list_deleted.json`](./tool-schemas/memory_admin_list_deleted.json).

```json
{
  "kinds": ["fact", "knowledge"],         // optional; events excluded automatically
  "since": "2026-05-01T00:00:00Z",         // optional; only items deleted at or after
  "author_id": "01HZ8Q...",                // optional; filter to one deleter
  "limit": 100,                             // optional; default 100, max 500
  "cursor": "..."                           // optional pagination cursor
}
```

**Output `_meta`**:

```json
{
  "results": [
    {
      "id": "01HZ8Q...",
      "kind": "fact",
      "content": { /* full PublicMemoryContent */ },
      "tags": [...],
      "author": { /* original author */ },
      "deleted_at": "<ISO-8601 UTC>",
      "deleted_by": { /* PublicAuthorRef of deleter */ },
      "created_at": "...",
      "updated_at": "..."
    }
  ],
  "next_cursor": "..." // null when exhausted
}
```

Errors: `INVALID_LIMIT`, `INSUFFICIENT_SCOPE`.
