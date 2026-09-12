# Sprint 049 — identities replace bearer tokens (header + whois)

korg proposal **2419**, slice 1 of program **2440** ("Simplify homelab
secrets"). Covered work items: **2388** (identities table keyed on a
declared `X-Homelab-Agent` header) and **2389** (`tailscale whois`
recorded beside the declared name, record-only).

Run as an overseen karc leg (`klams-1e7d4e`) on kubs0.

## Goal

klams's bearer tokens are name tags, not locks. Under the homelab threat
model — single user, his agents, one tailnet, agents already hold sudo
everywhere — a token's only job is to tell klams *which agent_name*. A
declared name does that without a secret, so there is nothing to rotate
and nothing to leak.

This slice adds the identities table and the whois record **with the
transition window open**: every existing `[[auth.tokens]]` row keeps
working, nothing is deleted, nothing is rotated. Closing the window and
deleting the token rows is slice **korg:2450**, after the four client
slices.

## Scope

In:

- `[[auth.identities]]` (`agent_name`, `scopes`, `label`, optional
  `nodes`) alongside `[[auth.tokens]]`.
- `X-Homelab-Agent: <agent_name>` authentication; unknown name → 401,
  exactly as an unknown token is today.
- Scope lookup by name; author binding unchanged (already keyed on
  `agent_name`).
- SIGHUP hot-reload covers the new table.
- `tailscale whois` of the caller's tailnet address, cached, recorded
  beside the declared name in the request log. `unknown` on failure;
  **never** refuses.
- Enforcement toggle in config, **default off**.
- An identity row for every current grant's `agent_name`, plus
  `klams-mind-eval` (`read`), which has no token row today.
- `docs/auth.md` rewritten for identities.
- Deployed on kubs0, with a live header write proving the author binding.

Out (belongs to korg:2450 or a client slice):

- Deleting `[[auth.tokens]]` rows or closing the window.
- Changing what this repo's own clients (`klams-client`, scanner,
  monitor, bench) *send* — they stay on bearer for this slice.
- The k-homelab age-store rows for klams grants (a k-homelab fold-in,
  filed from the wrap-up).

## Acceptance

1. A write with a valid `X-Homelab-Agent` header lands under the right
   author; an unknown name is refused; a read-only identity cannot write.
2. Both the header and legacy bearer work while `[[auth.tokens]]` rows
   exist.
3. whois is record-only and cached; `unknown` when tailscaled does not
   answer; a whois failure never refuses a request.
4. Enforcement toggle present and off by default; on + mismatched node →
   403 naming both.
5. New binary deployed on kubs0, service restarted, live header write
   recorded in the wrap-up by author and memory id.
6. `docs/auth.md` rewritten; `klams-token` still edits the table.

## Premise check (start of sprint)

All measured on kubs0, 2026-09-12, before any code changed.

| claim | verdict |
|---|---|
| 2388: `[[auth.tokens]]` is `token`/`scopes`/`label`/`agent_name` | **holds** — `klams-types/src/auth.rs` `TokenGrantConfig` |
| 2388: author binding is already keyed on `agent_name`, not token bytes | **holds** — `resolve_token_author` in `klams-service/src/main.rs` looks up `get_author_by_agent_name` and only registers on absence. Nothing to change. |
| 2388: `klams-token` is the config editor | **holds** — `tools/klams-token` |
| 2388: `docs/auth.md` carries "treat as a secret" framing | **holds** — including the "backups of this file are secret-bearing too" section |
| proposal: `klams-mind-eval` has no token row today | **holds** — 15 grants live, none named `klams-mind-eval` |
| 2389: peer address is the socket peer *or* `X-Forwarded-For` behind `tailscale serve` | **holds, and sharpened by measurement** — see below |
| 2389: `tailscale whois --json <addr>` identifies node and tags | **holds** — measured output shape below |

### The two measurements worth keeping

**klams is behind `tailscale serve` in production**, so the socket peer
is always `127.0.0.1` and the socket peer is never the useful fact.
`tailscale serve status` on kubs0 shows `https://kubs0…:7777 → proxy
http://localhost:7777`.

To find out what `serve` actually forwards rather than guess, a throwaway
listener was stood up on a spare port (`tailscale serve --bg
--https=8999`), curled **from kai** — the host whose requests this has to
identify — and torn down again (`tailscale serve --https=8999 off`,
confirmed gone). What arrived:

```
X-Forwarded-For: 100.97.109.60      <- kai's tailnet IP
X-Forwarded-Host: kubs0.encke-wahoo.ts.net:8999
X-Forwarded-Proto: https
```

No `Tailscale-User-*` headers, because kai is a tagged node and `serve`
only sets those for user-owned ones. So `X-Forwarded-For` is the single
source of the caller's tailnet address, and the socket peer is the
fallback for a direct (non-`serve`) deployment.

**`tailscale whois --json` output**, measured against a tagged node and a
user device:

| address | `Node.Name` | `Node.Tags` | `UserProfile.LoginName` |
|---|---|---|---|
| kai `100.97.109.60` | `kai.encke-wahoo.ts.net.` | `["tag:server"]` | `tagged-devices` |
| cleo `100.94.70.96` | `cleo.encke-wahoo.ts.net.` | `null` | `ken.hiatt@gmail.com` |
| unknown `100.64.0.99` | — | — | prints `peer not found`, not JSON |

The short node name is the first DNS label of `Node.Name`. An
unresolvable address prints a non-JSON line, so the parser must treat
"did not parse" and "command failed" identically — both are `unknown`.

## Decisions

Recorded as they are made.

### D-1 — The transition window closes by the absence of token rows, not a flag

WI 2388 offered either "a config flag (or the absence of any
`[[auth.tokens]]` rows)". Taking the second: the window is open exactly
while at least one `[[auth.tokens]]` row exists, and slice korg:2450
closes it by deleting the rows — which it is doing anyway. A flag would
be a second mechanism for one fact, and YAGNI says the slice that
actually needs a rollback switch is the one that should add it.

### D-2 — `X-Forwarded-For` is trusted for whois, and that is why enforcement ships off

The forwarded address is only as trustworthy as the proxy in front of the
service. klams on kubs0 is reachable through `tailscale serve`, but the
loopback port is also open to anything already on the host — so a local
caller could assert any `X-Forwarded-For` it likes.

That is fine for the recorded fact (2389 is explicitly record-only, a
debugging aid answering "why did X come from Y"), and it is precisely why
the enforcement toggle ships **off** with the limitation written into
`docs/auth.md` rather than on with a caveat. Turning it on is a decision
for a deployment that can show the loopback port is not reachable by
untrusted local callers; this slice does not make that claim.

### D-3 — `X-Homelab-Agent` beats `Authorization`, and an unknown name does not fall through

When both credentials are presented, the declared identity wins. If that
name is unknown the request is a 401 — it is **not** retried against the
bearer.

Falling through looks friendlier and is the worst outcome available: the
caller stated who it was, was wrong, and would be silently authenticated
as whoever its token names. Its writes would then be attributed to an
agent it never claimed to be, which is the one failure mode attribution
exists to prevent.

An empty or whitespace-only header is treated as "not presented" rather
than as an identity named `""`, so a client that sets the variable to
nothing falls back to its bearer instead of meeting a confusing 401.

### D-4 — enforcement is per-identity opt-in, and refuses a caller it cannot place

With `[auth.whois] enforce = true`, only identities that declare `nodes`
are constrained; an unpinned identity is unconstrained. That way turning
the toggle on cannot lock out every caller at once. `--validate-config`
warns about the identities it leaves unconstrained, because a config that
*looks* locked down and is not is worse than one that plainly is not.

Where enforcement is on and an identity *is* pinned, an unresolvable node
is a refusal. This is the single place a whois failure changes an
outcome, and it sits behind the default-off toggle: an operator asking
"prove where you came from" is not served by "could not tell".

## What shipped

**`klams-types`** — `IdentityConfig` (`agent_name`, `scopes`, `label`,
`nodes`), `WhoisConfig`, `AuthMethod`, `AuthenticatedPeer`. `AuthConfig`
grew `identities` and `whois`; `errors()` now accepts either table as
satisfying "at least one grant", refuses a duplicate `agent_name` inside
the identities table, and `warnings()` reports identities left
unconstrained under enforcement. The same-name-in-both-tables case is
explicitly *not* an error — it is the expected state for the whole
window.

**`klams-api`** — `whois.rs` (the `NodeResolver` trait, `TailscaleWhois`
with a TTL cache that holds negative answers, and a `parse_node_name`
pinned to fixtures measured off the live tailnet). `auth.rs` grew
`Identity`, `AuthTables`, and a `require_bearer` that checks the header
first and the bearer second, resolves the caller's node, optionally
enforces, stamps `AuthenticatedPeer`, and logs writes at `info` / reads
at `debug` with `agent_name`, `tailnet_node` and `auth`. New
`ApiError::NodeNotAllowed` → 403 `node_not_allowed`, naming identity,
actual node and allowed set.

**`klams-service`** — `build_auth_tables` resolves authors for both
tables through one shared `resolve_agent_author`, and validates the whole
`[auth]` block through `AuthConfig::errors()` rather than a second copy
of the rules. SIGHUP swaps both tables atomically. The accept loop stamps
`PeerAddr` (the fallback address; `X-Forwarded-For` is the primary).
Startup says out loud whether the transition window is open.

**`klams-token`** — an `identity` subcommand group (`list`, `add`,
`remove`, `scopes`, `nodes`). No `rotate` and no `--reveal`, because
there is no secret. The fingerprint-and-refuse guard now covers **both**
tables on **every** write, so a token edit that disturbed an identity row
— or the reverse — is refused rather than discovered.

**Docs** — `docs/auth.md` rewritten around identities (what a name tag
was, the window, whois, what enforcement trusts); `usage.md`,
`setup.md`, `architecture.md`, `klams-mcp-for-agents.md`, `README.md` and
the shipped example config follow.

## Repaired in passing

- **`klams-token identity nodes <x> --set ""` wrote `nodes = [""]`.**
  `--set ""` is the natural way to type "unpin", and clap hands it over
  as one empty string; taken literally that pins the identity to a node
  that cannot exist, which enforcement would then refuse every request
  against. Node lists are now trimmed and empty entries dropped, on both
  `add` and `nodes`. Found by the first CLI test run; covered by
  `identity_nodes_pins_and_unpins`.

- **The first `[[auth.identities]]` block rendered below `[postgres]`.**
  `next_position_in` fell through to end-of-document when the array did
  not yet exist — which is exactly today's live config. The result is
  valid TOML that reads like the file was mangled, and it is the failure
  the token path's positioning logic was written to avoid. It now falls
  back to the position of the *sibling* auth array before end-of-file.
  Covered at both layers
  (`adding_the_first_identity_to_a_tokens_only_config`,
  `identity_add_appends_a_row_and_leaves_every_token_grant_alone`).
