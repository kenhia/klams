# Authorization

How klams decides what a caller may do. Two things determine it: **which
identity the caller presents**, and the **scopes** granted to that
identity in `klams.toml`.

Scopes became load-bearing on both surfaces in sprint 025. Sprint 049
changed what a caller presents: a declared name instead of a secret.
Sprint 052 deleted the bearer path, so a name is now the *only* thing a
caller can present.

## Identity is not a secret

A klams bearer token was a **name tag, not a lock**. Under the homelab
threat model — one human, his agents, one tailnet, agents already
holding sudo on every host — the token's only job was to tell klams
*which `agent_name`* was calling. Nothing about it gated anything a
caller could not otherwise reach.

A declared name does that job without a secret. So a caller now sends:

```http
X-Homelab-Agent: claude
```

and klams looks the name up in `[[auth.identities]]`. An unknown name is
a `401`, exactly as an unknown token was. There is nothing to rotate,
nothing to leak, and nothing to register in a secret store.

**What this costs, stated plainly.** Anyone on the tailnet can write as
`claude`. Today anyone on the tailnet *holding a token* can, and korg has
run with no auth at all on the tailnet since July. The whois record
(below) makes a misconfigured or unexpected client visible. If a second
human ever joins the tailnet, the declared name becomes a JWT subject and
this table becomes an issuer's client list — the design does not
foreclose that.

**Attribution did not move.** klams has resolved `agent_name` → author
row at startup and keyed authorship on the *name* rather than the token
bytes since sprint 009. That was the part expected to be hard, and it was
already done: proven on 2026-07-31, when the klams-mind token was rotated
and `register_author` under the new token resolved to the pre-existing
author. Moving a consumer from a token to a header orphans nothing it
ever wrote.

### The transition window — CLOSED (sprint 050), and the code deleted (sprint 052)

The window was open exactly while `[[auth.tokens]]` had rows: there was
never a separate flag, because deleting the rows is what closes it.
**Sprint 050 deleted all fifteen.** While it was open both credentials
authenticated side by side, which is what let the consumers be cut over
one at a time with nothing breaking in between: sprint 049 opened it;
the four client repos (kmon, kyac, klams-view, klams-mind) each shipped
their half; 050 moved the clients klams itself owns — the scanner, the
monitor, the bench harness — plus the host MCP config files, and then
emptied the table.

**Sprint 052 deleted the code**, once the closure had been *observed*
from a restarted session on every host rather than merely believed (a
session already running keeps the config it loaded at start). There is
one check on every request:

1. `X-Homelab-Agent` against `[[auth.identities]]`. Present and unknown
   is a `401`; absent is a `401`.

A header that is present but **unknown** is a `401` and nothing falls
through — there is nothing left to fall through to. The caller said who
it was and was wrong; authenticating it as something else would
attribute its writes to an agent it never claimed to be.

**`Authorization` is still read, and authenticates nothing.** A request
carrying a bearer and no `X-Homelab-Agent` gets a `401` whose body names
the header to send and the one to drop:

```json
{"code": "bearer_retired",
 "message": "bearer tokens are retired (sprint 052): send `X-Homelab-Agent: <agent_name>` and drop the `Authorization` header"}
```

That distinction earns its keep. Three clients sat sending a retired
credential and getting a bare `401` for a full day (WI 2490) because,
from outside, "sent a retired credential" and "sent nothing" looked
identical. Sending nothing at all still gets the plain `unauthorized`.

## The one rule people get wrong

**Scopes are flat, not hierarchical.** An identity holding `write` does
*not* implicitly hold `read`. `admin` does not imply `write`. Every row
must list every scope it needs:

```toml
scopes = ["read", "write"]      # correct
scopes = ["write"]              # this identity cannot search
```

`Scope::satisfies` is exact equality
([`crates/klams-types/src/auth.rs`](../crates/klams-types/src/auth.rs)).
This is deliberate — it means granting a broad-sounding scope can never
silently confer a capability you didn't intend.

## Granting: `[[auth.identities]]`

Identities live in the `[auth]` block of `klams.toml` (see
[`deploy/config/klams.example.toml`](../deploy/config/klams.example.toml)).

**Use `klams-token` rather than an editor** (sprint 045, #265; extended
to identities in sprint 049). It edits these blocks structurally, so a
write cannot clobber a sibling — which is exactly how korg #264 happened
— and it validates the result against the types below before anything
reaches disk:

```bash
sudo klams-token identity list
sudo klams-token identity add krot --scopes read,write
sudo klams-token identity scopes krot --add manage
sudo klams-token identity nodes kmon --set kubs0     # see whois, below
sudo klams-token identity remove krot
```

There is no `identity rotate` and no `--reveal`, and that absence is the
feature: an identity has no secret to rotate or print.

`identity` is the only subcommand group. The legacy token subcommands
(`list`, `add`, `remove`, `scopes`, `rotate`) and `--reveal` were
deleted in sprint 052 and now fail as unrecognised rather than doing
something unexpected. Every write is still fingerprint-guarded: an edit
must produce exactly the change it declared and nothing else.

The shape:

```toml
[[auth.identities]]
agent_name = "claude"                        # the key; callers declare this
scopes     = ["read", "write", "manage"]     # non-empty, flat
label      = "claude"                        # for logs; optional
nodes      = ["kai", "kubs0"]                # optional pin; see whois
```

| Field | Required | Notes |
|---|---|---|
| `agent_name` | yes | 2–64 chars of `[a-z0-9_-]`. This is the row's **key**, not an optional binding — there is nothing else to look a row up by. Resolved to an author row at startup. Duplicates are refused rather than resolved by file order. |
| `scopes` | yes | Non-empty. See the table below. |
| `label` | no | Appears in startup logs; not security-relevant. |
| `nodes` | no | Tailnet nodes this identity may arrive from. Documentation until `[auth.whois] enforce = true`. |

Note what is *absent*: there is no `PrivilegedGrantNeedsAgentName` rule
here. Sprint 034 added it so every privileged action would be
attributable, and an identity row cannot be unattributable — the name is
the credential.

Recipes and the full write pipeline: [usage.md](usage.md#sprint-045--klams-token-auth-grant-cli).

## Recording where a caller came from: `[auth.whois]`

Authentication is by declared name, and a name says nothing about where
it was declared from. `tailscale whois` closes that gap: the caller's
tailnet address is resolved to a node and recorded beside the declared
`agent_name` in the request log, so *"why did a write from `claude` turn
up here"* is answerable after the fact.

```toml
[auth.whois]
enabled        = true     # resolve at all; turn off on a host with no tailscale
enforce        = false    # DEFAULT OFF — see below
cache_ttl_secs = 300
```

**It is data, not a check.** tailscaled being down, the `tailscale`
binary being absent, the address being unknown to the tailnet, and whois
being switched off all record `unknown`, and **none of them refuses a
request**. Resolved and unresolved answers are both cached, so an
unresolvable peer cannot make klams spawn a process per request.

Writes are logged at `info` and reads at `debug`, both carrying
`agent_name`, `tailnet_node` and `auth` (`identity` or `bearer`):

```
authenticated write agent_name=claude tailnet_node=kai auth=identity method=POST path=/mcp
```

That `auth` field is also how you read the transition window's progress:
while anything still logs `auth=bearer`, the window cannot close.

### Where the address comes from — and what enforcement trusts

klams runs behind `tailscale serve`, so the socket peer is always
loopback and carries no information. What `serve` forwards is
`X-Forwarded-For`, set to the caller's tailnet IP (measured on kubs0,
2026-09-12, by curling a throwaway `serve` listener from kai). klams
reads the first entry of that header, and falls back to the socket peer
for a direct, non-`serve` deployment.

**`enforce` ships off, and this is why.** A forwarded header is only as
trustworthy as the proxy in front of the service. klams's loopback port
is open to anything already on the host, so a local caller could assert
any `X-Forwarded-For` it likes. That is fine for the recorded fact — the
record exists to explain a misconfiguration, not to stop an adversary —
but it is not a foundation for a refusal. Turning `enforce` on is a claim
that the loopback port is not reachable by untrusted local callers; make
that claim deliberately.

Two more things about enforcement, both deliberate:

- **Pinning is per identity.** Only rows that declare `nodes` are
  constrained, so turning the toggle on cannot lock every caller out at
  once. `--validate-config` warns about the rows it leaves unconstrained,
  because a config that looks locked down and is not is worse than one
  that plainly is not.
- **An unplaceable caller is refused.** With enforcement on, a *pinned*
  identity whose node cannot be resolved gets a 403. An operator who asks
  "prove where you came from" is not served by "could not tell".

A refusal is `403 {"code": "node_not_allowed"}` naming the identity, the
node it actually arrived from, and what was allowed. A tailnet node name
is not a secret from a caller already on that tailnet, and an operator
debugging this needs all three.

### Backups of this file are not secret-bearing (sprint 050)

`/etc/klams/klams.toml` holds no secret. Its `[auth]` tables are a list
of names and scopes, so a backup of it is a list of names and scopes —
there is nothing in it to encrypt, and `klams-token` no longer tries.
Durable backups are plain timestamped copies beside the config, with the
same `0640 root:klams` mode as the config itself.

**This is a change of fact, not of posture.** Until 050 the file carried
fifteen live bearer tokens and every rotation minted another `.bak`
holding them; krot's grant inventory (klams #1377) found seven of them
in three naming conventions, several still carrying the *current* token
for most grants. Sprint 046 (#1384) answered that by encrypting durable
backups with `age` to a recipient kept off the homelab, with a plaintext
`{agent_name: sha256(token)[:12]}` manifest beside each so an audit
could ask "does this hold a live token?" without decrypting anything.

Sprint 050 removed all of it — the `age` encryption, the
`backup.age-recipient` file, `$KLAMS_TOKEN_AGE_RECIPIENT`, the manifests
and `klams-token restore --identity`. Encrypting a list of names, and
keeping a passphrase off-site to read it back, is machinery guarding
nothing. The pre-050 encrypted backups on kubs0 were deleted with the
token rows; they were only ever undo history for a table that no longer
exists.

Same-run rollback is unchanged and still needs nobody: a failed
validation restores from the in-memory copy `klams-token` already holds,
so a bad edit at 2am self-heals. What went is the off-site key.

The one thing the config still holds that *is* secret is the Postgres
URL under `[postgres]`, which carries the database password. That is why
`/etc/klams/` is `root`-only and why the rule for reading it is
`sudo klams-token identity list`, never `cat` or `grep` — a redaction
pattern written for token rows missed the `postgres://user:pass@host`
form on 2026-09-12 and printed the password into an agent transcript
(krot WI 2466).

### `[[auth.tokens]]` and `bearer_token` — RETIRED, and REFUSED at startup

Both are gone from the code (sprint 052; `bearer_token` had been refused
since sprint 034). A config that still names either **will not start**,
and `--validate-config` reports the same refusal:

```
/etc/klams/klams.toml: config names `[[auth.tokens]]`, retired in sprint 052:
delete the row and add an `[[auth.identities]]` row with the same `agent_name`
and `scopes` (`sudo klams-token identity add`); callers send
`X-Homelab-Agent: <agent_name>` instead of `Authorization: Bearer`
```

**Why a refusal rather than a silent ignore.** klams's config model
tolerates unknown fields, so deleting the struct fields alone would make
a surviving `[[auth.tokens]]` row vanish during parsing — and the
operator would go on believing a credential was live when it
authenticated nothing. klams therefore scans the raw file *before*
parsing it. Two properties of that scan are load-bearing:

- **Comments are stripped first.** The shipped example config documents
  both retired forms in prose, and so do several live configs. A guard
  that refused to start over a comment would fail exactly the operators
  it exists to protect.
- **Longest match first, and each match is consumed**, so the report
  describes the file rather than the pattern list.

(Both borrowed from kaed 024's D-1, which solved the same problem one
repo over.)

Migrating a row you find in an old config: drop `token`, keep
`agent_name`, `scopes` and `label`, rename the table to
`[[auth.identities]]`, and point the consumer at `X-Homelab-Agent`.
`agent_name` was always the part that mattered — it is what made a token
an identity, what `memory_delete` decides ownership by, and what
authorship has been keyed on since sprint 009. Dropping the token bytes
therefore orphans nothing.

An identity row has no `token` field and no minimum length, because
there is no secret. Everything else about the row is the same; see
[Granting](#granting-authidentities) above.

### Hot reload

Edit `[[auth.identities]]` and send `SIGHUP`; **the table swaps
atomically** with no restart and no dropped in-flight requests. Adding
or revoking an identity takes effect on the next request.

`[auth.whois]` is deliberately **not** hot-reloaded. The resolver owns a
cache and `enforce` decides whether a request can be refused; swapping
either under live traffic is a restart-shaped change.

```bash
sudo systemctl reload klams-service
```

`klams-token` prints this reminder after every write, and deliberately
does not run it: a config edit and a service action bundled together is
a bigger blast radius than that tool should take on.

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

## Recommended identity split

| Identity | Scopes | Why |
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

A legacy grant with no `agent_name` cannot delete anything, since
ownership is decided by the bound author. Give every one of them a name —
or better, replace it with an identity, where the name is mandatory.

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

`agent_name` must satisfy the same rule as an identity's: 2–64
characters of `[a-z0-9_-]`. Names like `"GitHub Copilot"` are refused
(with a suggested substitute) because no `[[auth.identities]]` row could
ever bind to them — and because the name travels in an HTTP header.

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
# Should be 403 scope_insufficient — a read-only identity must not write.
curl -s -o /dev/null -w '%{http_code}\n' -X POST \
  -H 'X-Homelab-Agent: klams-view' \
  -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:7777/memory/knowledge/index

# Should be 200 — the same identity reads fine.
curl -s -o /dev/null -w '%{http_code}\n' \
  -H 'X-Homelab-Agent: klams-view' \
  http://127.0.0.1:7777/memory/policy

# Should be 401 — an unknown name is refused like an unknown token.
curl -s -o /dev/null -w '%{http_code}\n' \
  -H 'X-Homelab-Agent: not-a-configured-name' \
  http://127.0.0.1:7777/memory/policy

# Should be 403 — a write identity must not resolve dissents.
curl -s -o /dev/null -w '%{http_code}\n' -X POST \
  -H 'X-Homelab-Agent: klams-scanner' \
  -H 'Content-Type: application/json' -d '{}' \
  "http://127.0.0.1:7777/memory/dissents/$SOME_ID/promote"
```

While the window is open the same probes work with
`-H "Authorization: Bearer $TOKEN"`, and comparing the two is how you
confirm a consumer is genuinely cut over rather than merely configured
to be.

## See also

- [setup.md](setup.md) — provisioning tokens, hot reload
- [usage.md](usage.md) — full endpoint and tool reference
- [klams-mcp-for-agents.md](klams-mcp-for-agents.md) — the agent-facing summary
- `sprints/025-authorization/sprint.md` — why the scope model looks the way it does
- `sprints/049-auth-identities/sprint.md` — why identities replaced tokens
