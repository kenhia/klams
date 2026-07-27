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
| ~~`INVALID_KIND`~~            | **Unreachable since sprint 018.** The flat `memory_add` schema makes `kind` a typed enum, so a bad value is rejected by deserialization as `SCHEMA_VALIDATION_FAILED` and this code can no longer be emitted. Kept listed for historical clients; do not match on it. (Drift found in the 2026-07-25 review F-3.1, corrected in 027.) | n/a |
| `INVALID_CATEGORY`            | `memory_append_event.category` empty                                       | No        |
| `INVALID_TOP_K`               | `top_k` ≤ 0 or > 50                                                        | No        |
| `INVALID_LIMIT`               | Admin list `limit` ≤ 0 or > 500                                            | No        |
| `EMPTY_QUERY`                 | `memory_search.query` empty                                                | No        |
| `SCHEMA_VALIDATION_FAILED`    | Fact `content.payload` failed per-type validator                           | No        |
| `EMBEDDING_UNAVAILABLE`       | The embedding backend is down or failing transiently (connect error, timeout, 5xx). Always carries `retry_after_seconds` | Yes |
| `PAYLOAD_TOO_LARGE`           | Sprint 027 (#629/#632). `memory_add.text` exceeds the embedding model's token ceiling. The message names the limit, the submitted size, and the character count to split below — enough to succeed on the *first* retry. **Permanent for that text**: never carries `retry_after_seconds` | No, not unchanged — split and resend |
| `EMBEDDING_REJECTED`          | Sprint 027 (#629). The embedding backend refused the request itself (a permanent 4xx that is not a size problem). Distinct from `EMBEDDING_UNAVAILABLE`: the service is healthy, the input is not | No |
| `NOT_FOUND`                   | Memory id does not exist. Since sprint 029, also: `memory_supersede`/`memory_update` on a superseded or deleted record (the message points at the replacement) | No        |
| `NOT_AGENT_AUTHORED`          | Sprint 029 (#638). `memory_supersede`/`memory_update` on scanner-ingested knowledge. Derived data updates via re-scan — fix the file instead; facts amend, events append | No |
| `NOT_SOFT_DELETED`            | `memory_admin_restore` on a row that is not currently soft-deleted         | No        |
| `EVENTS_NOT_DELETABLE`        | Delete tool called on an event id                                          | No        |
| `INSUFFICIENT_SCOPE`          | Caller's token lacks the required scope                                    | No        |
| `MAINTENANCE_WINDOW_ACTIVE`   | Backup window in progress; `_meta.retry_after_seconds` populated           | Yes, after `retry_after_seconds` |
| `INTERNAL_ERROR`              | Server-side bug or unexpected condition; details in logs. Carries `retry_after_seconds` **when and only when** the underlying failure was transient (e.g. database pool exhaustion) — sprint 027 | Only if `retry_after_seconds` is present |

**Retry guidance** (revised sprint 027, WI #629):

`_meta.retry_after_seconds` is present **if and only if** retrying the
identical call could succeed. That is now a property carried by the
error itself (`StoreError::is_transient`) rather than a guess made at
the call site, and it holds in both directions:

- A permanent failure — `PAYLOAD_TOO_LARGE`, `EMBEDDING_REJECTED`, any
  validation error — **never** carries a retry hint. Retrying it
  unchanged fails identically, forever.
- A transient failure — `EMBEDDING_UNAVAILABLE`,
  `MAINTENANCE_WINDOW_ACTIVE`, or an `INTERNAL_ERROR` whose cause was
  transient — **always** carries one.

Clients may therefore treat the presence of `retry_after_seconds` as the
authoritative retry signal and ignore the code's prose.

*Why this is spelled out:* both directions used to be wrong. A size
rejection was reported as `EMBEDDING_UNAVAILABLE` with
`retry_after_seconds: 5` — every signal telling the caller to wait and
retry something that could never work, which caused agents to conclude
the embedder was down and abandon the write (silent knowledge loss,
#629). Meanwhile a transient pool exhaustion arrived as a bare
`INTERNAL_ERROR` with no hint, telling callers to give up on something
that would have succeeded a moment later.

**Error code stability**: these codes are part of the public contract.
Renames require a spec amendment.
