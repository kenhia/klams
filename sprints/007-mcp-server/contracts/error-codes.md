# Canonical MCP Error Codes — sprint 007

Errors are returned in the MCP standard error envelope:

```json
{
  "isError": true,
  "content": [
    { "type": "text", "text": "human-readable message with remediation hint" }
  ],
  "_meta": {
    "error_code": "MISSING_AUTHOR_ID",
    "retry_after_seconds": null
  }
}
```

`_meta.error_code` is the machine-readable contract. Clients should
switch on this field, never on the text message.

| Code                          | Meaning                                                                    | Retryable |
|-------------------------------|----------------------------------------------------------------------------|-----------|
| `MISSING_AUTHOR_ID`           | Write tool called without `author_id` argument                             | No        |
| `UNKNOWN_AUTHOR_ID`           | Supplied `author_id` does not exist in the `authors` table                 | After `register_author` |
| `INVALID_AGENT_NAME`          | `register_author` called with empty or whitespace-only `agent_name`        | No        |
| `INVALID_REPO_PATH`           | `register_author` called with a non-absolute `repo`                        | No        |
| `EXTRA_TOO_LARGE`             | `register_author.extra` exceeds 16 KiB serialized                          | No        |
| `INVALID_KIND`                | `memory_add.kind` not one of `"fact" \| "knowledge"`                        | No        |
| `INVALID_CATEGORY`            | `memory_append_event.category` empty                                       | No        |
| `INVALID_TOP_K`               | `top_k` ≤ 0 or > 50                                                        | No        |
| `INVALID_LIMIT`               | Admin list `limit` ≤ 0 or > 500                                            | No        |
| `EMPTY_QUERY`                 | `memory_search.query` empty                                                | No        |
| `SCHEMA_VALIDATION_FAILED`    | Fact `content.payload` failed per-type validator                           | No        |
| `EMBEDDING_UNAVAILABLE`       | TEI adapter timed out / errored; client may retry                          | Yes       |
| `NOT_FOUND`                   | Memory id does not exist                                                   | No        |
| `NOT_SOFT_DELETED`            | `memory_admin_restore` on a row that is not currently soft-deleted         | No        |
| `EVENTS_NOT_DELETABLE`        | Delete tool called on an event id                                          | No        |
| `INSUFFICIENT_SCOPE`          | Caller's token lacks the required scope                                    | No        |
| `MAINTENANCE_WINDOW_ACTIVE`   | Backup window in progress; `_meta.retry_after_seconds` populated           | Yes, after `retry_after_seconds` |
| `INTERNAL_ERROR`              | Server-side bug or unexpected condition; details in logs                   | No        |

**Retry guidance**: `_meta.retry_after_seconds` is populated **only**
for `MAINTENANCE_WINDOW_ACTIVE` and `EMBEDDING_UNAVAILABLE`. Other
retryable conditions (network blips at the transport layer) are the
client's concern.

**Error code stability**: these codes are part of the public contract.
Renames require a spec amendment.
