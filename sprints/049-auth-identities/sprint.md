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

## A note on `path` in the request log

The audit line for an MCP call reads `path=/`, not `path=/mcp`. That is
the nested router reporting the path *within* its mount, and it is left
as it is deliberately: every MCP call is `POST /mcp`, so the path field
carries no information there in the first place, while REST calls log
their real path (`/memory/facts`, …). Nothing is lost, and changing it
would mean re-publishing an immutable store artifact for a cosmetic.

## Deployed 2026-09-12

- Version `0.1.49` live on kubs0 (`/healthz` confirms; was `0.1.48`).
  `klams-token --version` → `klams-token 0.1.49` (the binary `/healthz`
  cannot speak for, sprint 048 #1697).
- Published to the store as `artifacts/klams-{service,scanner,monitor,token}/0.1.49/`.
- Unit files: **unchanged** (`git diff -- deploy/` touched only
  `config/klams.example.toml`), so `install-systemd` was not run.
- kai's `klams-scanner`: **deployed to 0.1.49**. It was found at
  **0.1.45** — four releases behind, which is exactly the drift the
  deploy skill warns accumulates when a ship leaves kai out. Nothing in
  this sprint changes scanner behaviour; it was taken along to close the
  gap.
- Rollback target: `0.1.48` via `just rollback` (`.prev` binaries in
  place); any published version via `just deploy-from-store --version`.
- Migrations applied: **none** (this sprint adds no SQL migration).
- Config changes required: **yes, and made here** — 16
  `[[auth.identities]]` rows added to `/etc/klams/klams.toml` with
  `klams-token identity add`. No secret was handled: an identity row is
  a name, a scope list and an optional label. Fifteen mirror the live
  token grants exactly (verified field by field before writing); the
  sixteenth is `klams-mind-eval` (`read`), which has no token row and
  which klams-mind WI 2398 needs. All 15 `[[auth.tokens]]` grants are
  untouched and still authenticate.

### Verified live — and from which host

Every probe below was run **from kai**, not from kubs0, because the
fact being established is "a caller on another tailnet node can
authenticate and is recorded as coming from there". Running them
locally would have measured a different thing and recorded
`tailnet_node=kubs0`.

- **A header write lands under the right author.** `memory_add` over
  MCP as `X-Homelab-Agent: claude` wrote knowledge memory
  `01a09746-4953-7bc1-a15c-0729206ec141` under author `claude`
  (`019f4986-0ee3-7ae3-8de5-f697e2692dc6`).
- **Attribution provably did not move** — the strongest evidence for
  the whole design. Startup logged both:
  `bound bearer to author  agent=claude  author_id=019f4986-…` and
  `bound identity to author  agent=claude  author_id=019f4986-…`.
  The same author row, from both tables. Nothing `claude` ever wrote is
  orphaned by the cutover.
- **whois records the node**:
  `authenticated write agent_name=claude tailnet_node=kai auth=identity`.
- **Refusals**: unknown declared name → 401; no credential → 401;
  read-only identity (`klams-view`) → 200 on `GET /memory/policy` and
  403 on `POST /memory/knowledge/index`.
- **The window is open**: a legacy bearer still returns 200, and startup
  logged `sprint-049 transition window OPEN … Deleting those rows is
  what closes it (korg:2450)`. 16 identities bound at startup.
- `just health` and `just verify` pass (7 passed, 0 failed, 3 skipped);
  both units `active`, zero service ERROR lines, one expected
  `klams-monitor publish failed` at restart.
- Gate green, plus the full integration suite (`just test-integration`)
  against the docker stack, which was torn down afterwards.

## Deployed — post-merge confirmation (2026-09-12)

The deploy above ran **during implementation**, because the proposal made
it part of this slice's acceptance ("the new binary is deployed on
kubs0 … a live write with the header lands under the right author —
that is the proof"). This section closes the loop after the merge.

**No re-publish and no re-install were needed, and that is verified, not
assumed.** The only file that changed between the published build
(`a037508`) and merged `main` (`1b33421`) is `docs/install.md`:

```
git diff --name-only a037508 HEAD   →   docs/install.md
git diff --stat a037508 HEAD -- ':(exclude)*.md' ':(exclude)docs/**' ':(exclude)sprints/**'   →   (empty)
```

So the store's immutable `0.1.49` artifact **is** main's code. Per the
deploy skill's preflight step 8 that is the "skip the publish" case, not
the `--force` case.

Live state after the merge: `/healthz` → `0.1.49`, `klams-token
--version` → `0.1.49`, `Cargo.toml` → `0.1.49`.

### Re-verified from kai, after the merge

| probe | result |
|---|---|
| declared identity `claude`, `GET /memory/policy` | 200 |
| unknown declared name | 401 |
| read-only identity `klams-view`, write | 403 |
| no credential | 401 |
| `/healthz` (public) | 200 |
| legacy bearer (`klams-view`) | 200 — window still open |

And the whois record is already doing the job it was built for: over
three minutes the request log attributed **629** authenticated requests
to node `kai` and **1** to `cleo`. Before this sprint, "which host did
that write come from" had no answer at all.

- Rollback target: `0.1.48` via `just rollback`; any published version
  via `just deploy-from-store --version`.
- Migrations applied: none.
