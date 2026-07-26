# klams MCP — agent handoff

You (an AI coding agent — Claude Code, GitHub Copilot, or any
MCP-capable client) have been pointed at this document to wire
yourself up to **klams**, the homelab's shared memory store, and to
start using it as **cross-agent memory**. Everything you need is
below; ask Ken only for a bearer token if one hasn't been provided.

## The server

| | |
|---|---|
| Endpoint | `http://kubs0:7777/mcp` |
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
claude mcp add --scope user --transport http klams http://kubs0:7777/mcp \
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
      "url": "http://kubs0:7777/mcp",
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
      "url": "http://kubs0:7777/mcp",
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
