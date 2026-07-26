# Authorization

How klams decides what a caller may do. Two things determine it: the
bearer token presented, and the **scopes** granted to that token in
`klams.toml`.

Added in sprint 025, which made these scopes actually load-bearing on
both the REST and MCP surfaces.

## The one rule people get wrong

**Scopes are flat, not hierarchical.** A token holding `write` does
*not* implicitly hold `read`. `admin` does not imply `write`. Every
grant must list every scope it needs:

```toml
scopes = ["read", "write"]      # correct
scopes = ["write"]              # this token cannot search
```

`Scope::satisfies` is exact equality
([`crates/klams-types/src/auth.rs`](../crates/klams-types/src/auth.rs)).
This is deliberate — it means granting a broad-sounding scope can never
silently confer a capability you didn't intend.

## Granting: `[[auth.tokens]]`

Tokens live in the `[auth]` block of `klams.toml` (see
[`deploy/config/klams.example.toml`](../deploy/config/klams.example.toml)).

```toml
[[auth.tokens]]
token      = "claude-XXXXXXXXXXXXXXXXXXXX"   # ≥16 chars, treat as a secret
scopes     = ["read", "write", "manage"]     # non-empty
label      = "claude"                        # for logs; optional
agent_name = "claude"                        # binds an identity; optional
```

| Field | Required | Notes |
|---|---|---|
| `token` | yes | Minimum 16 characters. Compared in constant time. |
| `scopes` | yes | Non-empty. See the table below. |
| `label` | no | Appears in startup logs; not security-relevant. |
| `agent_name` | no | 2–64 chars of `[a-z0-9_-]`. Resolved to an author row at startup. Unset ⇒ writes attribute to the seeded `system` author. |

`agent_name` is what makes a token an *identity*, not just a key. It is
resolved to an `author_id` at startup and on every reload; every write
through that bearer is attributed to it, and — since sprint 025 —
`memory_delete` decides ownership by it.

### Hot reload

Edit `[[auth.tokens]]` and send `SIGHUP`; the grant table swaps
atomically with no restart and no dropped in-flight requests. Rotating
or revoking a token takes effect on the next request.

### The legacy `auth.bearer_token`

The single pre-sprint-007 `bearer_token` still works and materializes as
one grant carrying **all four** scopes. It is the "everything" token by
construction. Prefer per-purpose `[[auth.tokens]]` entries; keep the
legacy token, if at all, as an operator break-glass credential.

## What each scope authorizes

| Scope | Grants |
|---|---|
| `read` | Search, retrieval, listing, and every `GET`. Reads nothing into the store. |
| `write` | Creating memories, **and managing the ones this identity wrote** — including deleting them. |
| `manage` | Curating memories authored by *somebody else*: cross-author delete, and resolving dissents. |
| `admin` | Recovery operations: restore, hard-delete, list soft-deleted, and the author lifecycle verbs. |

### Why `manage` exists

The driving case is an agent that retrieves a memory, recognizes it as
wrong or stale, and removes it so the *next* agent isn't misled. That
agent is usually **not** the author — the scanners wrote most of the
store. Self-management alone would not serve that need, and handing
every writer cross-author delete would mean any scanner token could
empty the store.

So: writers curate their own records (a scanner re-reading a changed
file *should* be able to retract its stale chunk), and cross-author
curation is granted deliberately, per token.

`manage` is **not** implied by `admin`, and does not imply `admin` —
hard-delete and restore stay separate from everyday curation.

## Recommended token split

| Token | Scopes | Why |
|---|---|---|
| `viewport` | `["read", "write", "manage"]` | It edits and deletes facts, and it is where a human resolves dissents. |
| `scanner`, `kmon`, `klams-mind` | `["read", "write"]` | Write their own records; can retract them; cannot touch anyone else's. |
| `claude`, `ghcp` | `["read", "write", "manage"]` | Interactive agents that curate the corpus. |
| operator | all four | Used from your own shell, not wired into a service. |

> **The viewport is not read-only.** Docs before sprint 025 recommended
> a `["read"]` viewport token "so a UI compromise cannot mutate state."
> That was only nominally true — nothing enforced scopes on the fact and
> dissent routes, so the read-only token could mutate everything. Now
> enforcement is real, and a `["read"]` viewport gets 403 on its own
> curation features. What bounds the viewport is withholding `admin`:
> it cannot hard-delete, restore, or touch the author registry.

A grant with no `agent_name` cannot delete anything, since ownership is
decided by the bound author. Give every token one.

## Enforcement, by surface

### REST

Every protected route carries a `require_scope` layer
([`router.rs`](../crates/klams-api/src/router.rs)):

| Routes | Scope |
|---|---|
| all `GET`s, `POST /memory/search`, `POST /memory/context` | `read` |
| `POST /memory/facts`, `/memory/events`, `/memory/knowledge/index` | `write` |
| `POST /memory/knowledge/delete` | `write` |
| `POST /memory/dissents/:id/promote`, `/discard` | `manage` |

Refusals are `403` with `{"code": "scope_insufficient"}`, naming the
scope the token lacks. An unauthenticated request is `401` — the scope
layer sits *inside* bearer auth, not in front of it.

`POST /memory/knowledge/delete` requires a `machine` parameter. Omitting
it used to delete the path's chunks on **every** host.

### MCP

Tools are gated at dispatch *and* filtered from `tools/list`, so a token
never sees a tool it cannot call.

| Tools | Scope |
|---|---|
| `memory_search`, `memory_related`, `event_search` | `read` |
| `memory_add`, `memory_append_event`, `memory_delete`, `dissent_propose`, `register_author` | `write` |
| `memory_admin_*` (restore, hard-delete, list-deleted, list/remove/merge authors) | `admin` |

Refusals come back as tool results with `error_code:
INSUFFICIENT_SCOPE`, not transport errors.

## Ownership on `memory_delete`

Scope is only half the decision; the other half is who owns the record.

- **`author_id` is optional.** Omit it — the delete acts as the author
  bound to your token. This is the documented path.
- If supplied, it must **equal** your bound author. Naming somebody else
  is refused. You cannot act on another identity's behalf, with any
  scope.
- Deleting a memory **you wrote** needs only `write`.
- Deleting **anyone else's** needs `manage`.
- Knowledge points with no recorded author (legacy, pre-attribution) are
  treated as not-yours: curating them needs `manage`.

`deleted_by_author_id` records who performed every soft delete, so
cross-author curation leaves an audit trail.

> Before sprint 025 `author_id` was required but never checked, so any
> authenticated caller could pass any well-formed id — minting one via
> `register_author` if needed — and delete anything in the store. If you
> are reading older docs or agent instructions that describe passing an
> `author_id` to delete, they describe the hole, not the contract.

## Identities

`register_author` needs `write` (it mints identities; it was `read`
until sprint 025) and is **idempotent per `agent_name`** — a second call
returns the existing row rather than a new one.

You rarely need it. Token binding already attributes your writes; call
it only to write under a deliberately separate per-session identity.

`agent_name` must satisfy the same rule as a token grant's: 2–64
characters of `[a-z0-9_-]`. Names like `"GitHub Copilot"` are refused
(with a suggested substitute) because no `[[auth.tokens]]` grant could
ever bind to them.

Lifecycle verbs are `admin`-scoped:

- `memory_admin_list_authors` — every author with the counts that decide
  whether it is safe to remove, plus duplicate `agent_name`s.
- `memory_admin_remove_author` — refuses while the author owns anything
  (`AUTHOR_HAS_MEMORIES`). Never reassigns silently.
- `memory_admin_merge_authors` — reassigns facts, events, knowledge
  points, and soft-delete attribution from one author to another, then
  removes the source. Use this to collapse duplicates.

## Checking a deployment

Use a token you expect to be read-only — a dashboard or scrape
credential, **not** the viewport (which carries `write` and `manage`).

```bash
# Should be 403 scope_insufficient — a read-only token must not write.
curl -s -o /dev/null -w '%{http_code}\n' -X POST \
  -H "Authorization: Bearer $READ_TOKEN" \
  -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:7777/memory/knowledge/index

# Should be 200 — the same token reads fine.
curl -s -o /dev/null -w '%{http_code}\n' \
  -H "Authorization: Bearer $READ_TOKEN" \
  http://127.0.0.1:7777/memory/policy

# Should be 403 — a write token must not resolve dissents.
curl -s -o /dev/null -w '%{http_code}\n' -X POST \
  -H "Authorization: Bearer $SCANNER_TOKEN" \
  -H 'Content-Type: application/json' -d '{}' \
  "http://127.0.0.1:7777/memory/dissents/$SOME_ID/promote"
```

## See also

- [setup.md](setup.md) — provisioning tokens, hot reload
- [usage.md](usage.md) — full endpoint and tool reference
- [klams-mcp-for-agents.md](klams-mcp-for-agents.md) — the agent-facing summary
- `sprints/025-authorization/sprint.md` — why this model looks the way it does
