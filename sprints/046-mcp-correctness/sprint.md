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

Written at sprint start; decisions and surprises get appended as the
work happens.
