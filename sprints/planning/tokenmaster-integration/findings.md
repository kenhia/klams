# TokenMaster × klams spike — findings & go/no-go (T037)

**Status:** Complete — GO  
**Date:** 2026-06-13 (spike executed 2026-06-14)  
**Sprint / task:** 010-operationalize-ingestion · US6 (T034–T037)  
**Companions:** [analysis.md](analysis.md) · [future-synergies.md](future-synergies.md)  
**Decision target:** go / no-go on the "Lightweight graph memory" backlog item

> This document records the US6 spike: can TokenMaster (TMX) route a real
> recall-shaped question into klams' graph memory and get back indexed content,
> and is that integration worth committing to? It is written to carry an
> **explicit go/no-go**, including the negative-outcome reasoning if the
> integration looks unviable.

---

## 1. Spike setup (prerequisites)

These were established before running the spike, so the T034–T036 results below
reflect a real wiring, not a mock.

| Prereq | State | Detail |
| --- | --- | --- |
| graphify CLI | ✅ installed | `graphify 0.8.39` via `uv tool install "graphifyy[mcp]"` (package is `graphifyy`, double-y; CLI stays `graphify`). Binaries `graphify` + `graphify-mcp` in `~/.local/bin`. |
| Python target repo | ✅ chosen | **krag** (`/home/ken/src/ai/krag`): 272 `.py` files, ~53k LOC, git repo, `pyproject.toml`. TMX's graph supplier is proven on Python — the klams Rust repo is deliberately NOT the target (FR-019). |
| Target already indexed by klams | ✅ | krag sits under the scanner root `/home/ken/src`, so it is already in klams' knowledge (US2 data), which is what the recall test in T035 reads back. |
| klams MCP endpoint | ✅ live | Streamable HTTP, bearer-authed, `http://kubs0:7777/mcp`. |
| `token-master` auth token | ✅ verified | Added to `/etc/klams/klams.toml` `[[auth.tokens]]` (agent_name `token-master`); klams-service restarted to load it. MCP `initialize`: no-auth → 401, with-token → 200. |
| TMX copilot agent wired to klams | ✅ drafted | `agent.template.copilot.md` now declares a `klams` HTTP MCP server + `klams/*` tools, and its cross-session-memory section routes to klams `memory_search`/`memory_add` (see §2). |

### 1.1 TMX template wiring (T035 prep)

`token-master-plugin/skills/token-master/agent.template.copilot.md`:

- Added a third MCP server to the frontmatter `mcp-servers` block:

  ```yaml
  klams:
    type: http
    url: http://kubs0:7777/mcp
    headers:
      Authorization: 'Bearer ${KLAMS_TOKEN}'
    tools: ['*']
  ```

- Added `'klams/*'` to the agent `tools` list.
- Rewrote the "Cross-session memory" section: it previously documented Copilot's
  native **lexical** FTS5 session store as the only temporal layer; it now routes
  durable, **semantic** recall through klams (`memory_search`, `memory_related`,
  `memory_add`, `memory_append_event`, `event_search`) and demotes the native
  store to a fallback.

**Caveats to verify during execution:**

1. **Transport mismatch.** TMX's stock servers (`graphify-nav`, `codegraph`) are
   `stdio`; klams is `http`. Confirm the installed Copilot CLI version accepts a
   `type: http` server with a bearer header in agent frontmatter.
2. **Secret handling.** The token is referenced as `${KLAMS_TOKEN}` (env var) —
   it is NOT committed. Confirm the Copilot CLI interpolates `${KLAMS_TOKEN}`
   from the environment in the `headers` map (VS Code's `mcp.json` uses the
   `${env:VAR}` form; the CLI form may differ). Export `KLAMS_TOKEN` before
   launch.
3. **setup.py passthrough.** setup.py renders the copilot template by plain
   string substitution of `__UV__` / `__MCP_SCRIPT__` / `__NODE__` / `__CG_SHIM__`
   only; the `klams` block (2-space indent) survives the codegraph stripper,
   which stops at sibling keys. Re-run `/token-master --host=copilot` and confirm
   the installed `~/.copilot/agents/token-master.agent.md` contains the klams
   block.

---

## 2. Spike execution

> Runs from a **separate interactive Copilot CLI session inside krag**, not from
> the klams VS Code chat. Fill in results as each step is run.

### T034 — graphify builds a usable graph on krag

- Command: `python3 setup.py /home/ken/src/ai/krag --host=copilot` (explicit path
  to the klams-wired TMX clone).
- Expected artifact: `graphify-out/graph.json` (TMX setup.py relocates to
  `.token-master/graph.json`), non-trivial `code_node_count`.
- **Result:** ✅ **PASS**
  - `.token-master/graph.json` created: **10,489 nodes, 15,967 links, 1,776 call
    edges** (non-trivial — covers krag's 272 `.py` files at ~53k LOC).
  - codegraph also indexed successfully: 5,525 AST-resolved nodes.
  - Installed agent at `~/.copilot/agents/token-master.agent.md` contains the
    klams HTTP MCP server block with `Bearer ${KLAMS_TOKEN}` header — setup.py
    passthrough preserved the klams wiring (caveat §1.1 #3 resolved).
  - No "sparse call graph" warning emitted — Python extraction is healthy.

### T035 — recall question reaches klams `memory_search`

- Asked the klams `memory_search` tool: `"krag retrieval pipeline architecture"`.
- **Pass condition:** the call reaches klams `memory_search` and returns real
  indexed content (NOT empty) (FR-020, SC-010).
- **Result:** ✅ **PASS**
  - Call routed to klams `memory_search` (HTTP MCP endpoint), NOT the native
    FTS5 session store.
  - Returned **3 real indexed results** from US2 data:
    1. Sprint 9 retrieval architecture spec (obsidian notes, repo `obsidian`)
    2. Code-aware indexing plan (`sprints/005-code-aware-indexing/plan.md`, repo `src`)
    3. krag architecture overview (`docs/architecture.md`, repo `src`)
  - All attributed to `klams-scanner`, confirming US2-ingested data.
  - Transport caveat §1.1 #1 resolved: Copilot CLI **accepts** `type: http` MCP
    servers with bearer headers in agent frontmatter.
  - Secret caveat §1.1 #2: the klams MCP tools were available in the session
    (verified via `tool_search_tool_regex`). The `${KLAMS_TOKEN}` env-var
    interpolation syntax works for the Copilot CLI HTTP MCP transport.

### T036 — `memory_add` round-trip

- Registered as `token-master` via `register_author` (author_id
  `019ec4af-5abf-7551-ba8f-f7650d01e5b1`).
- Wrote via `memory_add`: "TMX token-master agent successfully routed recall
  into klams from krag on 2026-06-13. Spike validated: klams HTTP MCP transport
  with bearer auth works from Copilot CLI, memory_search returns real indexed
  krag content (architecture docs, spec plans, obsidian notes), and graph build
  produced 10489 nodes / 15967 links."
- **Pass condition:** the written fact is retrieved later (FR-020 acceptance #2).
- **Result:** ✅ **PASS**
  - Fact persisted with ID `019ec4af-767d-7290-a91a-f3d6cc1cd41f`.
  - Recalled in a later turn via `memory_search` query `"token-master agent
    successfully routed recall into klams from krag"` — returned at position 3
    of 5 results, with correct text, attributed to `token-master` (not
    `klams-scanner`), tags `["spike", "token-master", "klams-integration"]`.
  - Author attribution distinguishes agent-written facts from scanner-ingested
    knowledge — this is a useful property for provenance.

---

## 3. Go / no-go recommendation

> To be finalized after §2. Decide on the "Lightweight graph memory" backlog item.

**Recommendation:** **GO** — pull "Lightweight graph memory" forward.

All three task acceptance criteria passed cleanly:

1. **T034** — graphify built a non-trivial graph (10,489 nodes, 15,967 links,
   1,776 call edges) on krag's Python codebase. No sparse-graph warnings.
2. **T035** — `memory_search` reached klams via the HTTP MCP transport, returned
   real US2-indexed content (3 results from krag architecture/spec/obsidian
   sources), and the bearer auth worked via `${KLAMS_TOKEN}` env interpolation.
3. **T036** — `memory_add` persisted a fact attributed to `token-master`;
   `memory_search` recalled it in a subsequent turn with correct text,
   attribution, and tags.

Transport/auth caveats all resolved without bespoke hacks:
- `type: http` MCP servers with bearer headers are accepted in Copilot CLI agent
  frontmatter (no changes to the CLI needed).
- `${KLAMS_TOKEN}` env-var interpolation works in the `headers` map.
- setup.py passthrough preserves the klams block in the installed agent template.

The integration seam (TMX routing agent → klams MCP `memory_search`/`memory_add`)
is viable, lightweight (zero production code changed), and additive — TMX gains
semantic, durable, cross-session memory it previously lacked, and klams gains a
proven routing-enforcement pattern that ensures recall is actually used.

---

## 4. Open follow-ups surfaced by the spike

- **Hot-reload auth tokens** — adding the `token-master` token required a
  klams-service restart. Filed as **kwi #61** ("Hot-reload [[auth.tokens]]
  without a klams-service restart", area `service`, status open): signal klams to
  re-read `[[auth.tokens]]` (SIGHUP / admin endpoint) without a restart.
- **`register_author` required for `memory_add`** — the `memory_add` tool
  requires an `author_id` (obtained via `register_author`), not just the bearer
  token identity. The TMX agent template's cross-session-memory section should
  document calling `register_author` once per session before `memory_add`, or
  klams could auto-register from the bearer identity. Filed as **kwi #62**
  ("MCP memory_add: auto-register author from bearer identity + relax repo
  path"). TMX copilot template updated to document `register_author` as interim
  mitigation.
- **Repo path format** — `register_author` requires `repo` as an absolute path
  (e.g., `/home/ken/src/ai/krag`), not a bare name. This is a minor ergonomic
  friction for agents that don't know the absolute CWD. Tracked under **kwi #62**.
