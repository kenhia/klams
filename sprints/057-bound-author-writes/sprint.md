# Sprint 057 — the create tools write only as the caller's bound author

Proposal korg:4325, slice 1 of program korg:4327 (*Claude Duet writes
klams*). Run as an overseen karc leg (`klams-590e2d`) on kubs0. It covers
one work item, **#4323**. Version `0.1.57`.

## Goal

`memory_add`, `memory_append_event` and `dissent_propose` take an
optional `author_id`. WI #62 (sprint 018) filled an *omitted* one from
the caller's bound author, and said in so many words that "an explicit
author_id always wins". So any identity holding `write` could store a
memory, an irreversible event or a dissent attributed to another agent:
`register_author` any name, pass the id it returns. The curation tools
(`memory_delete`, `memory_supersede`, `memory_update`) have refused a
foreign `author_id` since sprint 025.

Ken ruled on 2026-11-02 that the fix is fleet-wide, not a per-identity
flag. The guard on that ruling is the survey below: if a live caller
depends on writing under a foreign author, stop and hand it back.

This has to be deployed before Claude Duet's identity is widened to
`write` (korg:4308). The korg-gate strips `author_id` from Duet's calls
(slice 2, korg:4326). This slice is the klams-side gate, so a gate bug
or a direct tailnet caller declaring `claude-duet` still cannot write as
anyone else.

## Survey: no live caller writes under a foreign author

I ran it before changing any behaviour, against the live store on
kubs0, 2026-11-02 ~20:50 PST.

**What is bound.** klams-service's startup log ("bound identity to
author", 2026-11-01 restart) lists 13 identities, each with its own
author: `ghcp`, `klams-scanner`, `klams-monitor`, `kyac`, `claude`,
`klams-mind`, `kai-scanner`, `klams-view`, `khound`, `kmon`,
`klams-mind-eval`, `klams-bench`, `claude-duet`.

**Postgres (facts, events, dissents).** I grouped every row by author
and checked it against that set (`.scratch-agent/057-bound.sql`). Rows
under an author that is not bound today:

| author | rows | latest |
|---|---|---|
| `ken_admin` | 5 facts | 2026-09-12 |
| `alice` | 6 facts | 2026-08-13 |
| `kyac` (5 old duplicate rows) | 5 events | 2026-07-10 |
| `GitHub Copilot`, `copilot-*`, `GHCP-*`, `controller` | 5 events | 2026-06-15 |
| `system` | 26 facts, 1 event | 2026-05-21 |

**Qdrant (knowledge, `knowledge_items_v2`).** A facet on `author_id`
shows 225,149 points under bound authors and 25 under unbound ones.
The newest unbound one is `ken_admin`'s, from 2026-09-12. Those 25 are
`klams-mind` (two pre-dedupe duplicate rows, July), `copilot-claude-opus-4.7`,
`ken_admin`, `alice`, `GitHub Copilot` ×2 and `token-master`.

Nothing in either store has been written under an unbound author since
2026-09-12. That is before sprint 050 retired the token table, which
was what made `alice`, `ken_admin` and `token-master` grants. Each of
those wrote under **its own** grant at the time. The duplicate `kyac`
and `klams-mind` rows are what `register_author` minted before sprint 025
made it idempotent.

**Callers that pass an id.** A grep of every repo on kubs0 and of
`~/.claude/skills` and `CLAUDE.md` turned up one: **klams-mind**. It
calls `register_author(agent_name=cfg.klams.agent_name)` and passes the
returned id to `memory_add` and `memory_delete`. It also sends
`X-Homelab-Agent: cfg.klams.agent_name`, the same name. Both lookups
resolve through `get_author_by_agent_name`, so the id is klams-mind's own
bound author and is accepted. It already passes the same way on
`memory_delete`, which has enforced this rule since 025.

The storage survey cannot see one case: a write under a *bound* author,
made by a *different* identity. The debug log line that would show it
is off at default verbosity, so this is a limit of the evidence and not
a finding. The caller grep is what covers that case, and it found
nothing.

**Verdict: holds.** The fleet-wide ruling stands.

## Premise check

- **MCP create tools take a foreign `author_id`: holds.**
  `crates/klams-mcp/src/tools/mod.rs` filled only the omitted case.
  `tests/mcp_identity_author.rs` had a test asserting
  `explicit_author_id_still_wins_over_the_callers_binding`.
- **"The REST routes that take an author": drifted. The hole was never
  there.** `POST /memory/facts`, `/memory/events` and
  `/memory/knowledge/index` stamp `Extension<AuthenticatedAuthor>`. Their
  request DTOs have no `author_id` field, and serde drops a
  client-supplied one (sprint 009 FR-010, with unit tests in
  `klams-types/src/requests.rs`). No REST route creates a dissent. So
  nothing on REST can be misattributed, and the attribution goal holds
  there already.

  What I did *not* do is turn REST's silent drop into a refusal. That
  would replace a documented contract (FR-010) with a new one. The
  ruling assumed REST took an author. It does not, so I put this to the
  overseer on the proposal rather than landing it quietly.

  **WI drift, ruled (overseer, korg:4325 comment 4298): no change.**
  FR-010 already meets the clause's intent, which is that REST cannot
  misattribute. Swapping a silent drop for a 403 would break a
  documented contract for no gain.

## What shipped

- `pin_bound_author` (`klams-mcp/src/tools/mod.rs`) replaces WI #62's
  fill at MCP dispatch, for all three create tools:
  - Omitted, `null`, nil or own id → pinned to the bound author.
  - Any other UUID → `INSUFFICIENT_SCOPE`, whatever the scopes. The
    wording follows `memory_delete`'s `acting_author`.
  - No bound author → `MISSING_AUTHOR_ID`. That is fail-closed. Before,
    an explicit id with no caller went through.
  - A non-UUID string → left for the tool's own `SCHEMA_VALIDATION_FAILED`.
- 8 unit tests beside it, including each tool's refusal and a foreign id
  in hyphenless spelling.
- Integration: `explicit_author_id_still_wins…` became
  `foreign_author_id_is_refused_on_every_create_tool` (all three tools,
  through a real MCP session) and `own_author_id_is_accepted`.
- **Tool descriptions changed.** The `author_id` schema field doc on all
  three tools now reads "If supplied it must equal that author — a
  different author's id is refused with `INSUFFICIENT_SCOPE` (sprint 057,
  WI 4323). Omit it." Before, it read "Pass explicitly to write as a
  different registered author." The `memory_add` and
  `memory_append_event` descriptions, plus the `dissent_propose`
  description (one appended sentence), now say "Omit author_id — the
  write is attributed to the author bound to your bearer token; another
  author's id is refused." korg-gate's copy `tests/klams_tools.json`
  will drift, and slice 2 regenerates it from the deployed klams.
- Docs: `docs/auth.md` has a new section on attribution for the create
  tools. In `docs/usage.md`, "explicit author_id always wins" is
  replaced.

## Gates

- `just gate`: green.
- `just test-integration`: 145 passed, 0 failed (test stack up and torn
  down on kubs0).
- The TDD order was loose. I wrote the guard and its unit tests in one
  pass, then turned the old "explicit wins" integration test, which
  would have failed, into the refusal test.

## Deployed 2026-11-02 (PST; 2026-11-03 05:17 UTC)

- Merged as PR #61 (`268954a`). CI on the PR was green; CI on the merge
  commit was in progress at deploy time.
- Version `0.1.57` is live on kubs0. `/healthz` reports `0.1.57` with
  `Ok` on every backend; the previous version was `0.1.55`, since 056
  did not deploy. `klams-token`, `klams-scanner` and `klams-monitor`
  each report `--version` 0.1.57.
- Published to the store as `artifacts/klams-*/0.1.57/` via
  `just publish`, then `just deploy-from-store` and `just restart`.
- Unit files: since 0.1.55, `deploy/klams-service.service` changed only
  in comments (056). The installed unit's directives match, so
  `install-systemd` was not run.
- kai's `klams-scanner` was left at 0.1.55 (probed from kubs0). 057
  touches only MCP dispatch, which the scanner does not use (it writes
  over REST).
- Rollback target: `0.1.55` via `just rollback` (the `.prev` binaries
  are in place), or `just deploy-from-store --version 0.1.55`.
- Migrations applied: none.
- Config changes required: none.
- `just health` as `claude`: 2 passed, 0 failed (light). Both units are
  active, and the journal shows no WARN/ERROR since the restart.

### Live proof (from kubs0, `X-Homelab-Agent: claude`, via `just mcp-call`)

| call | `author_id` | result |
|---|---|---|
| `memory_add` | kai-scanner's (`019f5903…`) | `INSUFFICIENT_SCOPE`, "memory_add writes as your own identity…" |
| `memory_append_event` | kai-scanner's | `INSUFFICIENT_SCOPE` |
| `dissent_propose` | kai-scanner's | `INSUFFICIENT_SCOPE` |
| `memory_add` | claude's own (`019f4986…`) | stored as `01a19fd7-a9e8-7613-91f9-40b355f9939b`, author `claude` |
| `memory_delete` | (omitted) | soft-deleted; Qdrant payload carries `deleted_at` and `deleted_by_author_id` = claude |

Postgres shows no stray proof rows: 0 events and 0 dissents carry the
proof markers.

**Found during the proof:** `memory_get` on the soft-deleted id still
returned the record. Its documented contract is `NOT_FOUND` for a
deleted id. The cause is `get_knowledge`, which does not check
`deleted_at`; this predates 057. I filed it as **WI 4331**. The decision
it needs is whether REST `GET /memory/knowledge/{id}`, which shares the
store call, should also hide deleted items.
