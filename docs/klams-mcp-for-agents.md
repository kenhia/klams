# klams MCP — agent handoff

You (an AI coding agent — Claude Code, GitHub Copilot, or any
MCP-capable client) have been pointed at this document to wire
yourself up to **klams**, the homelab's shared memory store, and to
start using it as **cross-agent memory**. Everything you need is
below; ask Ken only for a bearer token if one hasn't been provided.

## The server

| | |
|---|---|
| Endpoint | `https://kubs0.encke-wahoo.ts.net:7777/mcp` (from anywhere on the tailnet)<br>`http://localhost:7777/mcp` (on kubs0 itself) |
| Transport | MCP Streamable HTTP (rmcp; HTTP+SSE fallback on the same mount) |
| Auth | `Authorization: Bearer <token>` — required on every request |
| Server name | `klams-mcp` |

Tokens are `[[auth.tokens]]` entries in `/etc/klams/klams.toml` on
`kubs0`, each with a `scopes` list and an `agent_name` that writes
are attributed to — see
[setup.md § Token attribution](setup.md#token-attribution-agent_name).
Each agent should get its **own token** (read+write, distinct
`agent_name`) so its memories are attributable. Since sprint 018 a
token edit takes effect with `sudo systemctl reload klams-service` —
no restart.

## Enable it — Claude Code

**Global (user) setup** — available in every project; token stays out
of any repo. Recommended:

```bash
claude mcp add --scope user --transport http klams https://kubs0.encke-wahoo.ts.net:7777/mcp \
  --header "Authorization: Bearer <token>"
```

**Repo (project) setup** — only if a specific repo should pin its own
klams wiring. `.mcp.json` is committed, so reference the token via an
environment variable instead of inlining it:

```json
{
  "mcpServers": {
    "klams": {
      "type": "http",
      "url": "https://kubs0.encke-wahoo.ts.net:7777/mcp",
      "headers": { "Authorization": "Bearer ${KLAMS_MCP_TOKEN}" }
    }
  }
}
```

…and export `KLAMS_MCP_TOKEN` in the shell/user environment.

Verify with `claude mcp list` (or `/mcp` inside a session) — `klams`
should show connected, with `memory_search`, `memory_add`, etc. in
the tool list.

## Enable it — GitHub Copilot

**Workspace** — `<repo>/.vscode/mcp.json`, prompting for the token so
nothing secret lands in git:

```jsonc
{
  "inputs": [
    {
      "id": "klams-token",
      "type": "promptString",
      "password": true,
      "description": "klams bearer token"
    }
  ],
  "servers": {
    "klams": {
      "type": "http",
      "url": "https://kubs0.encke-wahoo.ts.net:7777/mcp",
      "headers": { "Authorization": "Bearer ${input:klams-token}" }
    }
  }
}
```

**Global (user)** — same `servers` block in the user-level MCP config
(VS Code: **MCP: Open User Configuration**; Copilot CLI:
`~/.copilot/mcp-config.json`, which uses the key `mcpServers` and
accepts a literal `headers.Authorization`). See
[setup.md § MCP server registration](setup.md#sprint-007--mcp-server-registration)
for the exact CLI shape and two known-harmless VS Code startup
warnings.

**Any other MCP client**: Streamable HTTP + the URL + the bearer
header is all there is; no OAuth, no session pre-registration.

## Using it (the part that matters)

The advertised tools are scope-filtered per token; a read+write token
sees `memory_search`, `memory_related`, `event_search`, `memory_add`,
`memory_append_event`, `memory_delete`, `dissent_propose`,
`register_author`. Full surface reference:
[usage.md § MCP server](usage.md#sprint-007--mcp-server).

Key behaviors (current as of sprint 018):

- **No registration dance.** `author_id` is optional on the write
  tools — omitted, the write is attributed to your token's
  `agent_name`. Call `register_author` only to write under a separate
  per-session identity.
- **Deleting (sprint 025).** Omit `author_id` on `memory_delete` too;
  it acts as your bound author. You can delete what you wrote. Deleting
  **another author's** memory needs the `manage` scope — passing their
  `author_id`, or a freshly registered one, does not work and never
  did what it appeared to. See [auth.md](auth.md).
- `memory_add` uses a **flat input schema** (`kind: "fact" |
  "knowledge"` discriminates; facts need `fact_type` + `payload`,
  knowledge needs `text`). No top-level `oneOf`, so Anthropic-bound
  agents can bind it directly.
- Writes land as `AgentProposal`; disagree with an existing canonical
  fact via `dissent_propose` rather than overwriting.

### Sprint 031 — MCP writes enforce the same policy as REST

Until 031 the MCP write path ran none of the checks the REST surface
ran. Three things changed, and all three are visible to a calling agent:

- **Fact payloads are validated.** `memory_add` now runs the same
  `ValidatorRegistry` REST has always run, so a payload that used to be
  accepted may now come back `SCHEMA_VALIDATION_FAILED`. The message
  names the offending field and rule (e.g.
  `payload.key (shape): key must match ^[A-Z][A-Z0-9_]*$`) — fix the
  payload from what it tells you rather than retrying unchanged.
- **`memory_add` takes an optional `amends`.** Pass the id of a fact you
  believe is now wrong and your payload replaces it. This is where the
  trust policy bites: if that fact came from a more-trusted source than
  an agent proposal, yours does **not** overwrite it. The call still
  succeeds, but the response carries `write_path: "dissent"` and a
  `dissent_id`, and the returned memory is the **canonical** record —
  what the store holds, not what you sent. Read that field before
  concluding your correction landed. Omit `amends` to add a new fact.
- **Identical knowledge dedupes.** Writing text that already exists
  (after whitespace normalization) returns the existing memory instead
  of creating a twin. Its `author` is whoever wrote it first, which may
  not be you. `similar_existing` still nudges you about *near*
  duplicates, where superseding is the right move.

### Instruction blurb — add this to the agent's instructions

Copy this (verbatim or trimmed) into the instructions file the agent
reads — `CLAUDE.md` / `AGENTS.md` / `.github/copilot-instructions.md`
— global if the MCP setup is global, per-repo otherwise.

**Write it as routing rules, not an offer.** These are phrased as
*enforced* steps ("do X FIRST, before Y"), on purpose. The lesson from
the TokenMaster integration is stark: an agent template that merely
*offered* a memory/structure tool saw it used 0 of 15 times; the same
tool phrased as a routing rule ("recall-shaped question → this tool
first") was used 8 of 8. Don't soften "FIRST" into "consider" — the
whole value of a shared memory is lost if agents don't reach for it
before grep/web.

> **Routing rule — klams is the shared cross-agent memory store** (MCP
> server `klams`). It holds durable homelab knowledge other agents and
> past sessions recorded: machines, services, past decisions, gotchas,
> conventions.
>
> - **Recall-shaped question → `memory_search` FIRST.** Before you grep
>   the repo, search the web, or ask "how/where/why/what is X" about the
>   homelab (a machine, service, past decision, error code, config key,
>   convention), call `memory_search` — *before* those other tools, not
>   after they come up empty. It is the cheapest source and often the
>   only one that has the answer.
> - **Learned something durable and non-repo-local → write it back.**
>   When you learn something that would help another agent or a future
>   session and isn't derivable from the repo you're in, `memory_add`
>   it — `knowledge` for prose findings, `fact` for structured
>   key/value state. Writes are attributed to your token automatically.
> - **Don't** store secrets, or anything already in the repo you're in.

Propagate this blurb to the instruction files on every machine an agent
runs from (currently `cleo`, `kai`, `kubs0`) so the routing rule is in
force everywhere, not just where it was first added.

## Smoke check

After setup, have the agent run `memory_search` with query
`"klams"` — non-empty scored results confirm the pipe end-to-end.
A `memory_add` (kind `knowledge`) followed by a `memory_search` for
its text confirms writes, and the new memory should appear in the
viewport attributed to the token's `agent_name`.

## Error codes — which ones are worth retrying

Every failure comes back as a structured envelope carrying a `code`,
a human `message`, and — **only when retrying can actually help** —
`retry_after_seconds`. That invariant is enforced in one place
(`klams_mcp::errors`, sprint 027 #629), so it is safe to branch on:

> **If `retry_after_seconds` is present, wait that long and retry.
> If it is absent, retrying will not help — fix the input or escalate.**

| Code | Retryable | What to do |
|---|---|---|
| `EMBEDDING_UNAVAILABLE` | **yes** (`retry_after_seconds: 5`) | The embedder is down or flapping. Wait and retry; the write was not stored. |
| `MAINTENANCE_WINDOW_ACTIVE` | **yes** | A backup is in flight. Wait and retry. |
| `INTERNAL_ERROR` | **sometimes** | Retry only if `retry_after_seconds` is present (transient backend trouble, e.g. pool exhaustion). Absent = escalate. |
| `PAYLOAD_TOO_LARGE` | no | The text exceeds the model's token ceiling. The message names the limit — **split the content and write the pieces**. Do not drop it. |
| `EMBEDDING_REJECTED` | no | The embedder refused the input for a non-size reason. Fix the input. |
| `EMPTY_QUERY`, `INVALID_TOP_K`, `INVALID_LIMIT`, `INVALID_WINDOW`, `WINDOW_TOO_LARGE`, `INVALID_KIND`, `INVALID_CATEGORY`, `SCHEMA_VALIDATION_FAILED`, `INVALID_AGENT_NAME`, `INVALID_REPO_PATH`, `EXTRA_TOO_LARGE` | no | Your arguments are wrong. The message says how. |
| `INSUFFICIENT_SCOPE` | no | Your token lacks the scope. Ask Ken; do not retry. |
| `NOT_FOUND`, `NOT_SOFT_DELETED` | no | The target isn't there (or isn't in the state the verb needs). |
| `NOT_AGENT_AUTHORED` | no | The target is scanner-ingested derived data. Fix the source file and let the re-scan update the store. |
| `EVENTS_NOT_DELETABLE` | no | Events are append-only by design. |
| `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID` | no | Omit `author_id` entirely — your token already identifies you. |
| `AUTHOR_HAS_MEMORIES` | no | Author removal refused; merge into another author first. |

The trap this table exists to close: before sprint 027 an oversize
payload came back as `EMBEDDING_UNAVAILABLE` **with** a retry hint, so
agents concluded the service was down, backed off, and silently dropped
the content instead of splitting it.

## Parameter limits

Exceeding one of these is a permanent error, not an outage.

| Where | Limit |
|---|---|
| `memory_search.query` | ≤ 1024 characters (`EMPTY_QUERY` if blank) |
| `memory_search.top_k` | 1..=50, default 10 |
| `memory_related.top_k` | 1..=50, default 5 |
| `event_search` window | ≤ `[api] memories_max_window_days` (default **30** days) |
| `memory_add` content | the embedding model's token ceiling — currently `max_input_tokens = 32768` (Qwen3-Embedding-0.6B). The service asks TEI's `/tokenize` for an exact count, so there is no character rule of thumb: 512 tokens is ~525 characters of prose but >20,000 of base64. |

## What a search result actually is

`memory_search` returns hits carrying a `score` that is a **Reciprocal
Rank Fusion score, not a cosine similarity**. Do not threshold it as if
it were one, and do not compare scores across queries — RRF values are
small (typically 0.01–0.07) and depend on how many sources returned the
item, not on how semantically close it is.

Ranking is: dense ANN over Qdrant + a curated stratum + Postgres
full-text, fused with RRF (sprint 024), then re-ranked by a
cross-encoder over the fused candidate set (sprint 030). **Rank order
is meaningful; absolute score is not.**

*(Sprint 032, #648: this section, the two tables above, and the tailnet
endpoint were all missing or wrong — agents had no way to tell a
retryable failure from a permanent one, no stated limits, and an
endpoint that only resolves on the LAN.)*

