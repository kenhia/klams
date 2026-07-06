# VS Code Insiders `"type": "http"` MCP handshake — research notes

Sprint 007 — Phase 9, T067.

VS Code (Insiders) speaks Streamable HTTP natively when an MCP server is
declared with `"type": "http"` in `.vscode/mcp.json`. Before issuing the
first JSON-RPC `initialize`, it tries to discover OAuth metadata and
will fall back to a "default auth metadata" path if the well-known
endpoints don't exist.

The notes below were captured against `klams-service` at commit
`add4e7b` + T064–T066 (bearer enforced on `/mcp`, scope-filtered
`tools/list`).

## 1. Client-side config used

[`.vscode/mcp.json`](../../.vscode/mcp.json):

```json
{
  "servers": {
    "klams": {
      "type": "http",
      "url": "http://kubs0:7777/mcp"
    }
  }
}
```

(Hostname `kubs0` is the operator's workstation; resolves via local DNS.
Token is not stored client-side — VS Code expects to acquire one via
the OAuth dance, or via a fallback prompt.)

## 2. Output panel log ("MCP: klams")

```
[info]    Stopping server klams
[info]    Starting server klams
[info]    Connection state: Starting
[info]    Starting server from Remote extension host
[info]    Connection state: Running
[warning] Could not fetch resource metadata: AggregateError: Failed to fetch resource metadata from all attempted URLs
[warning] Error populating auth server metadata for http://kubs0:7777: AggregateError: Failed to fetch authorization server metadata from all attempted URLs
[info]    Using default auth metadata
[info]    Waiting for server to respond to `initialize` request...
[warning] Error getting token from server metadata: Error: User did not provide client details
[info]    Received 403 status with Authorization header, retrying with new auth registration. Error details: Forbidden: Host header is not allowed
[info]    Waiting for server to respond to `initialize` request...
[warning] Error getting token from server metadata: Error: User did not provide client details
[info]    Connection state: Error 403 status sending message to http://kubs0:7777/mcp: Forbidden: Host header is not allowed
```

## 3. Observations

1. **OAuth metadata probes (RFC 9728 / RFC 8414)** — VS Code attempts
   GETs against the resource (no exact URL surfaced in the panel, but
   the warnings name "resource metadata" and "authorization server
   metadata"). Standard well-known paths are:
   - `GET <base>/.well-known/oauth-protected-resource` (RFC 9728)
   - `GET <base>/.well-known/oauth-authorization-server` (RFC 8414)
   - and per-MCP convention also
     `GET <base>/.well-known/oauth-protected-resource/mcp`.

   None exist on `klams-service`, hence the two `AggregateError` lines.

2. **"Using default auth metadata"** — with no metadata served, VS
   Code falls back to a built-in default that drives a dynamic client
   registration / authorization-code flow. Because no client details
   were ever issued ("User did not provide client details"), token
   acquisition fails immediately.

3. **First `initialize` POST hits the host allowlist, not auth** —
   VS Code does send the initialize anyway. The 403 it receives reads
   `Forbidden: Host header is not allowed`. That string originates in
   rmcp's `StreamableHttpService` host-allowlist check, which previously
   was hardcoded to `["localhost","127.0.0.1","0.0.0.0"]` in
   [crates/klams-mcp/src/transport.rs](../../crates/klams-mcp/src/transport.rs).
   The operator was connecting via hostname `kubs0`, so the request was
   rejected *before* `require_bearer` even ran.

4. **VS Code misinterprets the 403** — because the response carries no
   `WWW-Authenticate` header, the client assumes a stale credential
   and "retries with new auth registration", looping the same 403.

## 4. Required server-side changes

| # | Change | Status |
| - | ------ | ------ |
| a | Make rmcp's `allowed_hosts` configurable (so operators with non-loopback hostnames aren't blocked). Default empty = disable check; `require_bearer` is the real gate. | DONE — `ServerConfig.mcp_allowed_hosts` (`[server]` TOML). Wired through `klams_mcp::router(state, allowed_hosts)`. |
| b | Decide OAuth posture (T068): Path A — advertise an *empty* OAuth Protected Resource metadata + return `401 WWW-Authenticate: Bearer realm="klams"` so VS Code surfaces a manual token-paste dialog; or Path B — serve static OAuth metadata pointing at a no-op authorization server and accept a client-credentials flow. | PENDING T068 |
| c | Once posture is decided, implement `/.well-known/oauth-protected-resource` (and `/.well-known/oauth-authorization-server` if Path B) at the service root, *not* under `/mcp`. | PENDING T069 |
| d | Switch `/mcp` denials from rmcp's 403 to `klams_api::require_bearer`'s 401 with a proper `WWW-Authenticate` header so VS Code's "auth needed" branch trips correctly. | Half-done — `require_bearer` already returns 401, but it does **not** emit `WWW-Authenticate`. Add header in T069. |
| e | Live VS Code Insiders smoke test after T069 — confirm Output panel reaches "tools/list received" without OAuth dialog. | PENDING T070 |

## 5. Next handshake to capture

After fix (a) is deployed, the OAuth metadata 404s will still occur
(expected — that's what T068/T069 fix), but the subsequent `initialize`
POST should reach `require_bearer` and return `401`. Re-capture the
Output panel after the operator restarts the service and click
**Restart** on the klams server entry. The new log will tell us
whether VS Code:

- prompts the user for a bearer token (Path A is sufficient), or
- silently fails because no OAuth metadata is advertised (need Path B
  or an explicit metadata stub).

Paste the new log into §6 below for T068 to land.

## 6. Post-fix-(a) Output panel log

```log
2026-05-25 10:43:51.013 [info] Stopping server klams
2026-05-25 10:43:51.055 [info] Starting server klams
2026-05-25 10:43:51.055 [info] Connection state: Starting
2026-05-25 10:43:51.065 [info] Starting server from Remote extension host
2026-05-25 10:43:51.072 [info] Connection state: Running
2026-05-25 10:43:51.123 [warning] Failed to parse message: ""
2026-05-25 10:43:51.174 [warning] Failed to parse message: ""
2026-05-25 10:43:51.175 [info] Discovered 6 tools
```

Corresponding `.vscode/mcp.json` entry:

```jsonc
"klams": {
  "type": "http",
  "url": "http://kubs0:7777/mcp",
  "headers": {
    "Authorization": "Bearer <write-tier-token>"
  }
}
```

## 7. Conclusion — Path A wins by construction

VS Code Insiders' `"type": "http"` MCP client **already supports static
bearer auth out of the box via `headers`**. With an explicit
`Authorization` header in `mcp.json`:

- The two `/.well-known/*` 404s are non-fatal — VS Code warns and
  proceeds.
- `initialize` succeeds; the bearer flows through `require_bearer`.
- `tools/list` returns the scope-filtered surface (6 tools for a
  read+write token, 9 for admin, 3 for read-only).
- **No OAuth dialog, no client registration, no token-paste prompt.**

The two `Failed to parse message: ""` warnings correspond to rmcp's
SSE keep-alive pings (empty `data:` lines, same ones our test
harness skips in [crates/klams-service/tests/mcp_auth.rs](../../crates/klams-service/tests/mcp_auth.rs)).
They are cosmetic. If they become annoying, switch the MCP service to
`json_response = true` in
[crates/klams-mcp/src/transport.rs](../../crates/klams-mcp/src/transport.rs);
that returns plain JSON for single-response calls and skips the SSE
framing. Defer unless an operator complains.

### Decision (T068)

**Path A, minimal form:** ship as-is. Document the
`headers.Authorization` pattern in
[sprints/007-mcp-server/quickstart.md](quickstart.md) §4 and
[docs/setup.md](../../docs/setup.md). No OAuth metadata endpoint is
required for VS Code; the OAuth probe failures are silent warnings.

### Optional polish (consider for T069, NOT blockers)

| # | Polish | Why |
| - | ------ | --- |
| i | Emit `WWW-Authenticate: Bearer realm="klams"` from `require_bearer`'s 401 path. | If an operator forgets to set `headers`, VS Code could in theory drive a token-prompt flow. Currently low payoff because VS Code accepts manual `headers`. |
| ii | Serve a minimal `/.well-known/oauth-protected-resource` returning `{"resource": "<base>/mcp", "authorization_servers": []}`. | Silences the two `Could not fetch resource metadata` warnings in the Output panel. Pure cosmetics. |
| iii | Flip `json_response = true` to drop SSE keep-alive pings (eliminates the `Failed to parse message: ""` warnings). | Cosmetics; may also slightly simplify the test harness parser. |

Recommendation: skip i/ii/iii for sprint 007 unless an operator
complains. Re-open as a polish task in a follow-up sprint if needed.
T069 collapses to a no-op; T070 is already implicitly passed (tools
populate without a dialog); T071/T072 update docs to reflect the
`headers` pattern and the new `[server].mcp_allowed_hosts` knob.
