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

**Use `klams-token` rather than an editor** (sprint 045, #265). It
edits these blocks structurally, so a write cannot clobber a sibling
grant — which is exactly how korg #264 happened — and it validates the
result against the types below before anything reaches disk:

```bash
sudo klams-token list                    # never prints token values
sudo klams-token list --verify           # which grants does the service still accept?
sudo klams-token add krot --scopes read,write --reveal
sudo klams-token rotate klams-scanner    # keeps agent_name, so nothing is orphaned
```

Recipes and the full write pipeline: [usage.md](usage.md#sprint-045--klams-token-auth-grant-cli).

### Backups of this file are secret-bearing too

**A backup of a secret-bearing file is itself a secret-bearing surface:
encrypted at rest, or registered and retained deliberately.** krot's
grant inventory (klams #1377) found seven `klams.toml.bak-*` files in
`/etc/klams` in three ad-hoc naming conventions, several still holding
the **current** live token for most of the 14 grants — the same
`0640 root:klams` exposure as the config itself, with none of the
attention. Every rotation minted another one.

Since sprint 046 (#1384) `klams-token` encrypts its durable backups with
`age`, to a recipient whose private half is passphrase-protected and
kept **off the homelab filesystem**:

```bash
# Ken, off-homelab, once:
age-keygen | age -p > ken-klams-backup.age   # keep this OFF kubs0
# the PUBLIC half goes on the host:
sudo tee /etc/klams/backup.age-recipient <<<'age1…'
```

`klams-token` finds that file beside the config on its own. (A
`--age-recipient` flag and `$KLAMS_TOKEN_AGE_RECIPIENT` override it; the
file is the primary route because these commands run under `sudo`, which
drops the environment.) **With no recipient configured, backups stay
plaintext and every write says so loudly** — refusing to edit the config
because backups cannot be encrypted would turn a hardening feature into
an outage.

Two things make this affordable:

- **Auto-restore still works without Ken.** The same-run rollback — the
  restore-on-failed-validate the write pipeline has always done — uses
  the in-memory copy the tool already holds. No plaintext outlives the
  operation, and a failed validate at 2am self-heals with nobody awake.
  Only the durable `.bak` on disk is encrypted.
- **A plaintext manifest sits beside each backup**, carrying
  `{agent_name: sha256(token)[:12]}` and nothing else, so krot and any
  audit can still answer "does this backup hold a live token?" without
  decrypting anything or learning a value.

Reading one back:

```bash
sudo klams-token restore /etc/klams/klams.toml.bak-20260827T120000Z.age --identity -   # prints it
sudo klams-token restore <backup> --identity - --apply                                  # makes it live
```

`--identity -` reads the age identity from stdin, so it never lands on
this filesystem. `--apply` goes through the same validated write pipeline
as any other mutation, so putting an old config back cannot itself break
the service.

**Losing the passphrase loses only undo history.** The live config and
the k-homelab secret store are the primaries.

The hand-edited shape it produces:

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
| `agent_name` | see notes | 2–64 chars of `[a-z0-9_-]`. Resolved to an author row at startup. **Required when `scopes` includes `manage` or `admin`** (sprint 034, #703) — privileged actions must be attributable. Optional otherwise; unset ⇒ writes attribute to the seeded `system` author. |

`agent_name` is what makes a token an *identity*, not just a key. It is
resolved to an `author_id` at startup and on every reload; every write
through that bearer is attributed to it, and — since sprint 025 —
`memory_delete` decides ownership by it.

### Hot reload

Edit `[[auth.tokens]]` and send `SIGHUP`; the grant table swaps
atomically with no restart and no dropped in-flight requests. Rotating
or revoking a token takes effect on the next request.

```bash
sudo systemctl reload klams-service
```

`klams-token` prints this reminder after every write, and deliberately
does not run it: a config edit and a service action bundled together is
a bigger blast radius than that tool should take on.

### The legacy `auth.bearer_token` — RETIRED (sprint 034)

The single pre-sprint-007 `bearer_token` is **retired**. It
materialized one grant carrying all four scopes with no `agent_name` —
an unattributable privileged credential, which is exactly what the
sprint-034 rule above forbids. Its history, for the record: sprint 032
(#670) stopped provisioning one by default and migrated kubs0 to
scoped grants only; sprint 034 (#703) closed the path entirely.

**Migration note.** A config that still sets `bearer_token` refuses to
start (and refuses `--validate-config`, and a SIGHUP reload keeps the
previous table) with an error pointing here. This is deliberate: the
key is still *parsed* precisely so it can be refused loudly — silently
ignoring a credential the operator believes is live would surface as
unexplained 401s instead. To migrate, express the same capability as an
attributed grant:

```toml
[[auth.tokens]]
token      = "<your old bearer_token value>"
scopes     = ["read", "write", "manage", "admin"]
label      = "break-glass"
agent_name = "operator"
```

Same power, but every action through it lands in the audit trail under
`operator` instead of vanishing into `system`.

## What each scope authorizes

| Scope | Grants |
|---|---|
| `read` | Search, retrieval, listing, and every `GET`. Reads nothing into the store. |
| `write` | Creating memories, **and managing the ones this identity wrote** — including deleting them. |
| `manage` | Curating memories authored by *somebody else*: cross-author delete/supersede/update (sprint 029), and resolving dissents. |
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

### Resolving dissents without a UI

A `manage` token's other job is settling dissents — the corrections
agents file with `dissent_propose`, and the trust-tier collisions the
write path diverts rather than overwrites. **There is no UI for this
as of sprint 039**: the viewport was the curation surface and it was
retired, and [klams-view](https://github.com/kenhia/klams-view) is
read-only by design (dissent actions sit in its roadmap). Until that
lands, resolution is three REST calls:

```bash
# List what is pending (read scope).
curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:7777/memory/dissents | jq

# Inspect one (read scope).
curl -s -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:7777/memory/dissents/$ID | jq

# Settle it — manage scope. Promote makes the proposed correction
# canonical; discard drops it and leaves the incumbent standing.
curl -s -X POST -H "Authorization: Bearer $MANAGE_TOKEN" \
  http://127.0.0.1:7777/memory/dissents/$ID/promote
curl -s -X POST -H "Authorization: Bearer $MANAGE_TOKEN" \
  http://127.0.0.1:7777/memory/dissents/$ID/discard
```

Nothing expires a pending dissent, so an unattended store simply
accumulates them; the list endpoint is the queue.

## Recommended token split

| Token | Scopes | Why |
|---|---|---|
| `klams-view` | `["read"]` | The dashboard only reads. Give it nothing else. |
| `scanner`, `kmon`, `klams-mind` | `["read", "write"]` | Write their own records; can retract them; cannot touch anyone else's. |
| `claude`, `ghcp` | `["read", "write", "manage"]` | Interactive agents that curate the corpus. |
| operator | all four | Used from your own shell, not wired into a service. |

> **A note on UI tokens, because this advice reversed twice.** Before
> sprint 025 the docs said "give the UI a `["read"]` token so a UI
> compromise cannot mutate state" — which was only nominally true, since
> nothing enforced scopes on the fact and dissent routes and that
> read-only token could mutate everything anyway. Sprint 025 made
> enforcement real, and the advice flipped: the `viewport` desktop app
> needed `["read", "write", "manage"]`, because it *was* the curation
> surface — a `["read"]` viewport got 403 on its own features.
>
> Sprint 039 retired the viewport in favour of
> [klams-view](https://github.com/kenhia/klams-view), which is
> deliberately read-only, so `["read"]` is right again. The rule
> underneath all three positions is the same: **scope the token to what
> the client actually does.** The client changed; the posture followed.
>
> Curation itself did not disappear — it just has no UI right now.
> Dissents are resolved over REST with a `manage`-scoped credential
> (see [Resolving dissents without a UI](#resolving-dissents-without-a-ui)).

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
| `memory_search`, `memory_get`, `memory_related`, `event_search` | `read` |
| `memory_add`, `memory_append_event`, `memory_delete`, `memory_supersede`, `memory_update`, `dissent_propose`, `register_author` | `write` |
| `memory_admin_*` (restore, hard-delete, list-deleted, list/remove/merge authors) | `admin` |

Refusals come back as tool results with `error_code:
INSUFFICIENT_SCOPE`, not transport errors.

## Ownership on `memory_delete` / `memory_supersede` / `memory_update`

Scope is only half the decision; the other half is who owns the record.
All three verbs ride one gate (`authorize_curation`, sprint 029 —
supersession *is* a delete plus a write, and update is a rewrite):

- **`author_id` is optional.** Omit it — the delete acts as the author
  bound to your token. This is the documented path.
- If supplied, it must **equal** your bound author. Naming somebody else
  is refused. You cannot act on another identity's behalf, with any
  scope.
- Acting on a memory **you wrote** needs only `write`.
- Acting on **anyone else's** needs `manage`.
- Knowledge points with no recorded author (legacy, pre-attribution) are
  treated as not-yours: curating them needs `manage`.

`deleted_by_author_id` records who performed every soft delete —
including supersessions, which additionally record `superseded_by` —
so cross-author curation leaves an audit trail. `memory_update` never
changes a record's author: a `manage`-tier edit of another author's
memory edits *their* record, it does not adopt it.

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
credential such as `klams-view`'s, **not** an interactive agent's
(which carries `write` and `manage`).

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
