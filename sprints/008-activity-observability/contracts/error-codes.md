# Error Codes — sprint 008 additions

Two new entries added to the canonical error-code list. Both follow
the stability promise from
[sprint 007's error-codes.md](../../007-mcp-server/contracts/error-codes.md):
codes are public contract surface; renames require a spec amendment.

Existing error codes from sprint 007 are unchanged.

---

## New codes

| Code                | Surfaces                     | Meaning                                                                                | Retryable | `_meta` keys                  |
|---------------------|------------------------------|----------------------------------------------------------------------------------------|-----------|-------------------------------|
| `INVALID_WINDOW`    | `event_search` MCP, `GET /v1/memories` | Request had `since > until`. Client must fix the order.                       | No        | none                          |
| `WINDOW_TOO_LARGE`  | `event_search` MCP, `GET /v1/memories` | `(until - since) > api.memories_max_window_days`. Client must shrink the window or paginate by windows. | No | `window_max_days: u32` |

Both codes are returned in:

- The standard MCP error envelope (`isError: true`, with `_meta.error_code` and `_meta.window_max_days` for `WINDOW_TOO_LARGE`).
- The HTTP `400` body: `{"error_code": "...", "message": "...", "window_max_days": 30}` (last field omitted for `INVALID_WINDOW`).

---

## Why these codes are distinct

- `WINDOW_TOO_LARGE` deserves its own code (not a generic `INVALID_ARGUMENT`) because the remediation is specific and machine-actionable: "shrink your window or paginate".
- `INVALID_WINDOW` is its own code because the remediation differs: "fix the order of `since` and `until`". Bundling the two would force clients to parse the human message to distinguish.

---

## Examples

### MCP — `WINDOW_TOO_LARGE`

```json
{
  "isError": true,
  "content": [{
    "type": "text",
    "text": "Requested window of 45 days exceeds the configured maximum of 30 days. Shrink the window or paginate."
  }],
  "_meta": {
    "error_code": "WINDOW_TOO_LARGE",
    "window_max_days": 30
  }
}
```

### MCP — `INVALID_WINDOW`

```json
{
  "isError": true,
  "content": [{
    "type": "text",
    "text": "Window is inverted: `since` (2026-05-25T00:00:00Z) is after `until` (2026-05-24T00:00:00Z)."
  }],
  "_meta": {
    "error_code": "INVALID_WINDOW"
  }
}
```

### HTTP — `WINDOW_TOO_LARGE`

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
  "error_code": "WINDOW_TOO_LARGE",
  "message": "Requested window of 45 days exceeds the configured maximum of 30 days.",
  "window_max_days": 30
}
```

### HTTP — `INVALID_WINDOW`

```http
HTTP/1.1 400 Bad Request
Content-Type: application/json

{
  "error_code": "INVALID_WINDOW",
  "message": "Window is inverted: `since` is after `until`."
}
```
