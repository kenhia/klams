# Sprint 046 — MCP correctness + token hygiene

- **korg proposal**: [korg:1662](korg:1662) — "Sprint: klams MCP
  correctness + token hygiene"
- **Work items**: #850, #853, #1230, #869, #1178, #1377, #1384, #859
- **Branch**: `046-mcp-correctness` · **Version**: `0.1.46`

## Goal

Fix the klams MCP surface where it bites agents daily, then clear the
token-hygiene tail that has been accumulating since #1377.

Five of the eight items are the MCP surface itself. The headline is
**#850**: `memory_add(kind=fact)` and `dissent_propose` have been
uncallable from any MCP client for months — long enough that Ken's
global `CLAUDE.md` documents the failure as a standing workaround
("unreachable from Claude Code … every call fails
`SCHEMA_VALIDATION_FAILED`"). A bug that has graduated into
documentation is the right thing to lead a sprint with.

## Scope

### #850 — untyped payload fields (bug, S)

`MemoryAddArgs::payload` and `DissentProposeArgs::proposed_payload` are
bare `Option<serde_json::Value>` / `serde_json::Value`. schemars renders
those with no `type`, the client therefore never sends a JSON object,
and the `payload.is_object()` guard refuses the call before it reaches
any store logic.

This is the *same* defect WI #309 fixed for
`memory_append_event.payload` in sprint 019. The fix was applied to one
call site instead of to the class, and the regression test written then
(`no_boolean_property_subschemas_anywhere`) does not catch these two:
`Option<serde_json::Value>` renders as an untyped **object** schema
(`{"description": …}`), not as the boolean `true` that test looks for.
So the guard was aimed one notch too narrowly and the sibling fields
walked straight under it.

The fix is one `#[schemars(with = …)]` attribute per field. The
*sprint's* job is the class rule: a test asserting that **no advertised
property is left unconstrained** — no `type`, no `enum`, no
combinator — so the next free-form field added cannot reacquire this.

### #853 — stale MCP instructions (chore, XS)

The `instructions` block in `get_info()` is delivered on every
connection to every agent and cannot be checked by its reader, so a
wrong sentence there is paid for constantly. Two are wrong:

- *"call `register_author` only to write as a separate per-session
  identity"* — sprint 025 made it idempotent on `agent_name`; it dedupes
  and returns the token-bound author. It cannot produce the separate
  identity the sentence warns about. This sentence caused the
  2026-07-25 rpidash3 misdiagnosis (handoff `korg:635`).
- *"consider superseding that memory instead of writing a
  near-duplicate"* — `similar_existing` arrives on the response to a
  write that has **already happened**. "Instead of" is not an action the
  caller still has.

Rule worth carrying forward, from the WI: **when a sprint changes a
tool's contract, the instructions block is a call site.**

### #1230 — log `tools/list` (chore, XS)

`call_tool` logs `mcp.tool dispatch`; `list_tools` logs nothing — so the
one operation the korg:1212 failure class actually breaks is the one
klams cannot observe. Cost sprint 043 an extra verification pass. One
`tracing::info!` recording tool count, negotiated protocol and whether
cache metadata was emitted. Deferred out of 043 only because 0.1.43 was
already published; this sprint ships a binary, so it lands here.

### #869 — SSE drops every ~80s (bug, Unknown)

**Time-boxed.** First measurement is the one the WI names: hit the
loopback origin directly, bypassing ts.net. That single test separates a
missing server-side SSE keepalive from a tailscale-serve idle timeout.
If it turns into a rabbit hole, it gets parked with findings rather than
eating the sprint.

### #1178 — compact-by-default `memory_search` (feature, M)

Port khound's response contract: snippet + locator + explicit fetch op,
per-source caps, diagnostics on demand. Measured 4,491 → 1,024–1,476
tokens per answered query on the same suite with answers preserved.
Four parts, all named in the WI: compact hits; an MCP **fetch-by-id**
tool (the REST side exists — the MCP surface is the gap, and without it
compact responses are strictly worse than full text); **match-window**
snippets rather than head-of-text; and measurement against the khound
eval harness rather than vibes.

Default-on is what the evidence and korg's precedent both argue, but it
changes every existing consumer's contract — a deliberate migration
call, recorded here, not a silent flip.

### #1377 / #1384 — token backup hygiene (chore XS + feature S)

#1377's ask (2) shipped in 045. What remains is ask (1): the one-time
manual prune of the legacy plaintext `/etc/klams/klams.toml.bak-*`
files, several of which still hold **current live tokens** in three
ad-hoc naming conventions. `klams-token`'s pruner deliberately refuses
to touch backups it did not write, so this pass is manual by design.

#1384 then makes the class safe: age-encrypt the durable backups to a
Ken-held recipient. The constraint that shapes the design is that
**auto-restore must keep working without Ken** — the same-run
transactional rollback uses the in-memory copy the tool already holds,
and only the durable `.bak` on disk is encrypted. A failed validate at
2am still self-heals. Plus a plaintext fingerprint manifest beside each
backup so krot can reason about contents without decrypting, and a
`klams-token restore` subcommand so restore is a command rather than an
improvisation.

### #859 — weekend residue (chore, XS)

Trim `.claude/skills/deploy-kubs0/SKILL.md` to what is genuinely
klams-specific and point at the shared `sprint-ship` skill for the ship
mechanics; drop the `sprints/<branch>/sprint.md` filename override
(de-hardcoded in agent-skills #669). Settle the uncommitted `reload`
justfile recipe on the kubs0 clone.

Note the WI corrects the infra-cleanup plan's §7.8 claim that the two
skills *disagree* about `[skip ci]`: verified on the machine, they do
not. It is duplication, not conflict.

## Sequencing

#850 + #853 together (the schema fix and the instructions that describe
it), then #1230 while in the same file, then #1178 (the M). Hygiene tail
anywhere. #869 time-boxed.

## Acceptance

- A Claude Code session can write a fact and file a dissent over MCP
  with no payload special-casing, and a test locks the *class*.
- The instructions block describes the post-025 world.
- `tools/list` leaves a log line naming what shape was served to which
  revision.
- `memory_search` returns compact hits by default with a one-call
  fetch-by-id beside it, measured on the frozen suite.
- No plaintext live-token backup remains in `/etc/klams`, and new
  durable backups are encrypted.
- Gate green, integration stack green, docs updated in-sprint.

## Log

### #850 was three fields, not two

The class test found `register_author.extra` — a third instance the WI
had not spotted, sitting in the same shape (`serde_json::Value` with
`#[serde(default)]`, rendered as `{"default": null}`: no `type`, so
unsendable as an object). That is the argument for the class rule
making its own case on the first run. It also had to be given a `{}`
default so the advertised default stopped contradicting the advertised
type; behaviour is unchanged, since the store already normalized a null
`extra` to `{}` before insert.

### #869 is a klams bug, and it is not SSE-specific

Diagnosed in full and fixed; it did not need the time-box.

The WI's proposed first test — loopback vs ts.net — was run and was
useful mostly by elimination: an idle keep-alive connection to the
loopback origin is closed at **30s**, the `header_read_timeout`, not at
80s. Neither observed timer was the tailscale proxy's, which pointed
back into this repo.

The cause is `IdleTrackedIo` in `crates/klams-service/src/limits.rs`.
It advanced its idle clock only in `poll_read`, so "idle" meant *the
client has not spoken*, not *nothing is happening*. An SSE stream is
server → client only. No amount of server traffic moved the clock —
**including rmcp's own 15s SSE keepalive pings, whose entire purpose is
to hold the stream open** — so the watchdog evicted a connection that
had been busy the whole time.

The arithmetic settles it. `keep_alive_timeout_secs` defaults to 75 and
the watchdog ticks at `keep_alive / 8`, so eviction lands in
75s..=84.4s. The uptimes reported in the WI were 77, 79, 79, 81 and 83.

Fix: count traffic in both directions. Genuinely idle connections are
still evicted on the same schedule — T1 and T2 pass unchanged; the only
difference for them is that the clock now starts at the response write
rather than the request read, microseconds apart.

Worth noting what this was *not*: not a missing keepalive (rmcp sends
them), and not the proxy. The server was sending exactly the frames
that should have kept the connection alive, and was killing the
connection for not hearing them come back.

### #1178 measured, and the harness needed replacing to do it

The WI says to measure with khound's eval harness. That harness could
not answer the question: **its klams adapter talks to REST**, and this
sprint deliberately left REST alone (the token argument is an
agent-context argument; REST has non-agent consumers). Running it would
have reported no change and been true.

So the measurement is a small non-shipping tool,
`tools/response-tokens`, over the same frozen suite
(`suite-002.toml`, sha256 `4b2ea7aa…`) and the same live corpus. It
projects each real search response into both wire shapes using **the
snippet code that actually ships**, and it keeps khound's rule that
makes the comparison honest: when the snippet did not carry what the
answer key wanted, it charges a follow-up `memory_get`. Without that
rule "compact" would just mean "truncated", and truncation always wins
a token comparison while losing the thing the tokens were for.

Result over 41 queries, 19 answered, top_k 10
(`token-measurement.txt`):

| | tokens per answered query |
|---|---|
| full text | 9,599 |
| compact | 4,193 |
| | **2.29× reduction** |

**Follow-up reads charged: 1 of 19.** That is the number that decides
whether the contract is a win, and it says the match-window snippet is
carrying the answer 18 times out of 19.

These absolutes are not comparable to khound's 4,491 — different token
accounting and a corpus several months further along. The *relative*
result on the same suite under the same rule is the claim.

The one query that forced a fetch, `con2-ports`, is the self-reference
fixture the suite header flags as a deliberate contaminant.

### #859: one ask was already done

The uncommitted `reload` recipe the WI describes is **not** uncommitted —
it landed in sprint 042 (`e3d9c44`) and the working tree was clean at
sprint start. The WI was written 2026-08-01 against a clone whose drift
has since been resolved; nothing to do but verify and say so.

The skill dedupe stands and is done. Note the WI's own correction held
up: the two skills do **not** disagree about `[skip ci]` — they agreed,
which is exactly when a restatement is pure liability. The shared skill
has since grown a hazard note (the marker is contagious through a quoted
PR body) that the local copy never had, so the local copy was already
the worse of two identical rules.

### #1384 landed; #1377's prune is Ken's

#1384 is built and documented: durable backups age-encrypt to a
configured recipient, with a plaintext fingerprint manifest beside each
one, and `klams-token restore` reads them back. The design constraint
that shaped it is that **auto-restore must keep working without Ken** —
so the same-run rollback moved to the in-memory copy the writer already
held, and a test proves it by deleting the durable backup mid-flight.

It is inert until Ken generates the keypair off-homelab and drops the
public half in `/etc/klams/backup.age-recipient`. Until then backups
stay plaintext and every write says so loudly, because refusing to edit
the config when encryption is not configured would turn a hardening
feature into an outage.

#### The prune, done

The reconciliation the WI comment asked for first is in
`backup-inventory.md`. The count is **seven**, not five, and six held
tokens the running service still accepts — the newest held 13 of 14.
The outlier `bak-1783644937` enumerated zero grants because it predates
the multi-token schema; it carried the retired `bearer_token` instead,
which made it the one a fingerprint sweep would most easily have missed.

Ken created `/etc/klams/backup.age-recipient` mid-sprint (identity
generated off-fleet, passphrase-protected, password manager + USB), so
the prune could finish properly rather than leaving one plaintext file:

- the newest backup was encrypted to that recipient as
  `klams.toml.bak-20260817T021051Z.age`, with its plaintext fingerprint
  manifest beside it;
- all seven plaintext files were **`shred -u`**ed, not `rm`ed — these
  held live credentials, and unlinking leaves the blocks.

`/etc/klams` now holds **no plaintext token backup**. #1377's premise is
gone.

**One verification is Ken's and cannot be done here** — by design.
Nothing on kubs0 can decrypt that backup, so nobody here has confirmed
the ciphertext is readable with the real identity. If the recipient were
a valid-but-wrong key, the failure would be silent and would only
surface when it mattered. Worth one round trip:

```sh
sudo klams-token restore /etc/klams/klams.toml.bak-20260817T021051Z.age --identity -
```

What is lost if that fails is undo history only — the live config and
the k-homelab store are the primaries, which is exactly why the trade
was worth making.

#### Also noticed

`monitor.env.bak-20260813-202310` is a backup of `monitor.env` in a
sixth ad-hoc convention, left in place as outside #1377's scope. The
generalizing principle now in `docs/auth.md` applies to it too.

### The integration stack earned its place again

`just gate` passed clean through all of this and missed three real
breakages, all of them found only by the docker-compose suite:

- `mcp_auth`'s three scope-surface pins, which enumerate the exact tool
  list each scope sees. Adding `memory_get` at Read scope is precisely
  the change they exist to make someone look at.
- two `mcp_lifecycle_verbs` assertions and one in `mcp_rerank` that
  reach into `hit["memory"]["id"]` over the wire — the full shape the
  compact contract replaced.

That is AGENTS.md's "before pushing anything that touches the store, the
MCP tools, or the write paths" clause doing exactly its job. This sprint
touched all three.

The lifecycle test was rewritten to assert on the compact snippet **and
then call `memory_get`** for the full record, so the fetch op is now
exercised end-to-end over the wire rather than only in unit tests.

### Follow-ups

- **Ken: verify the encrypted backup decrypts.** Nothing on kubs0 can,
  by design. One `klams-token restore … --identity -` settles it.
- `monitor.env.bak-20260813-202310` is a plaintext backup of
  `monitor.env` in a sixth ad-hoc convention — outside #1377's scope,
  but the same principle applies.
- REST `/memory/search` still returns full records. That was the right
  call for this sprint; whether the compact contract should reach it is
  a separate decision with non-agent consumers to consider.

## Deployed 2026-08-28

- Version `0.1.46` live on kubs0 (`/healthz` confirms `status: Ok`, all four
  backends `Ok`; was `0.1.45`).
- Published to the store as `artifacts/klams-{service,scanner,monitor}/0.1.46/`.
- Unit files: unchanged (`git diff 366f4c1..HEAD -- deploy/` is empty), so
  `install-systemd` was not run and the scanner timer was not disturbed.
- kai's `klams-scanner`: **left at its current version.** Nothing this sprint
  touches the scanner — the changes are the MCP tool surface, the service's
  connection watchdog, and `klams-token`. Deliberate, per the skill's "decide
  explicitly" rule.
- **`klams-token` needed a separate install and nearly got missed.**
  `just deploy-from-store` ships three binaries; `klams-token` is not one of
  them, so `/usr/local/bin/klams-token` was still `0.1.45` after the deploy —
  *without* backup encryption. The next grant edit would have minted a
  plaintext backup and quietly undone #1377's prune. Fixed with
  `just install-klams-token`; now reports `0.1.46` and carries `restore` and
  `--age-recipient`. **Worth folding into the deploy skill.**
- Rollback target: `0.1.45` via `just rollback` (`.prev` binaries in place);
  any published version via `just deploy-from-store --version`.
- Migrations applied: **none** (`git diff 366f4c1..HEAD -- migrations/` empty),
  so `just rollback` is a complete undo here.
- Config changes required: none to `klams.toml`. Ken added
  `/etc/klams/backup.age-recipient` during the sprint, which is new but not a
  `klams-service` config change — the service never reads it.

### Verified live, beyond `/healthz`

Each of the four MCP-surface items was exercised against the deployed
instance, three of them through a real Claude Code MCP client rather than a
synthetic one:

- **#850** — `memory_add(kind: "fact")` **succeeded from a Claude Code
  session**, writing `01a046e7-…` (deleted again after). This is the WI's
  acceptance criterion verbatim, and the store shows no agent had written a
  fact since 2026-05-28.

  The *first* attempt is the better evidence: it failed with
  `payload.key (shape): key must match ^[A-Z][A-Z0-9_]*$` — a **store
  validator** complaint about the key's casing. Before this sprint the same
  call died at `payload must be a JSON object`, before reaching any store
  logic at all. The payload now arrives as an object; the only thing left to
  argue about is its contents.

- **#1178** — `memory_search` returned compact hits over MCP: `snippet` (match-
  windowed, elided with `…`), `id`, `score`/`raw_score`/`source_rank`,
  `age_seconds`, `tags`, `author`, and `more: {"fetch": "memory_get",
  "truncated": true}`. Typed metadata is omitted where it does not apply —
  `repo` appeared on exactly the one hit that has one.

- **#1230** — the log line is live:
  `mcp.tools/list tools=11 protocol=2026-07-28 cache_metadata=true`.
  Eleven tools is the write-scope surface, which now includes `memory_get`.

- **#869** — an authenticated SSE stream against the deployed service held
  **135 seconds** with 51 keepalive bytes flowing, clean through the
  75..=84.4s window that used to evict it. Before this sprint the same stream
  died three times inside that span and the client gave up on the transport.

- **#1384** — the deployed `klams-token 0.1.46` advertises `restore` and
  `--age-recipient`, and `/etc/klams` holds one age-encrypted backup plus its
  plaintext manifest and no plaintext config backup at all.

Units settled: `klams-service` and `klams-monitor` both `active`, **zero**
ERROR/WARN lines in the service log since restart, and exactly one
`klams-monitor` publish failure at startup — the known race, not a regression.

### Note for the next agent: the tools/list cache delays this fix

This session's MCP client was still serving the **0.1.45 tool catalog** after
the restart — its cached `memory_add.payload` schema carried no `type`, and
`memory_get` was absent from the catalog entirely. The write above worked
anyway, because the payload shape is decided by what the client *sends* and
this client sent an object regardless.

But it means the #850 fix reaches an agent only when its cached catalog
expires or it reconnects. Sprint 043 set `ttlMs` with `cacheScope: private`
precisely so clients would cache this. **An agent still seeing
`SCHEMA_VALIDATION_FAILED` after 0.1.46 should restart its MCP client before
filing anything** — and Ken'''s global `CLAUDE.md`, which documents the bug as
a standing workaround, is now stale and worth correcting.
