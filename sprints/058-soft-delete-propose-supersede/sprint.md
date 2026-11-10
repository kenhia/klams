# Sprint 058 — `memory_get` honours soft-delete, and a typed propose-supersede path

Proposal korg:4373, a slice of program korg:4368 (the November 2026
backlog drawdown). Run as an overseen karc leg (`klams-94341e`) on kubs0.
Covers **WI 4331**, plus the klams half of **klams-mind WI 3331**, which
lives in another project and so cannot be covered here. Version `0.1.58`.

## Goal

1. **WI 4331.** `memory_get` documents `NOT_FOUND` "when no live memory
   carries that id", but served a soft-deleted knowledge record in full.
2. **klams-mind WI 3331.** Ken decided the authority question on
   2026-10-23 (the propose-merge path): klams-mind files merge and
   duplicate proposals into a klams queue and never supersedes another
   author's memory itself; an identity with `manage`, or Ken in bulk,
   accepts or rejects, and acceptance performs the supersede with lineage.
   Decisions 2 (retiring the second record) and 3 (overlapping clusters)
   were handed to the sprint that builds the path. This is that sprint.

## Acceptance

- `memory_get` returns `NOT_FOUND` for soft-deleted knowledge and facts,
  with a test.
- The REST decision is recorded (below).
- A propose-supersede tool with fully typed arguments, an acceptance
  path, and retirement of several records by one accepted proposal.
- The new tools are proven callable from a Claude Code MCP client.

## Premise check (sprint start)

- **WI 4331 — premise drifted, direction holds.** The knowledge leak
  reproduced in a hermetic test before the fix. But two of the brief's
  asks were already true: facts were never affected (Postgres
  `fetch_facts_with_authors` filters `deleted_at IS NULL`), and
  `memory_update` / `memory_supersede` have refused a deleted id since
  sprint 029 (`point_is_soft_deleted` → `NOT_FOUND`). So the fix shrank
  to the one read path, plus a fact test that locks the existing
  behaviour in.
- **klams-mind WI 3331 — premise holds.** Nothing propose-supersede-shaped
  existed in klams-mcp.

## Decisions

**D1 — the `memory_get` filter lives in `memory_by_id`, not the store.**
`QdrantStore::get_knowledge` is shared: REST `GET /memory/knowledge/{id}`,
`memory_update`, `memory_supersede` and retrieval call it. The update
verbs already check for deleted ids themselves. Retrieval filters in its
Qdrant query. REST GET is the read that restore tooling uses. Filtering
in the store would have taken that read away to fix a bug that has one
caller. **REST GET keeps serving soft-deleted records** (the overseer's
call, recorded here as the WI asked). `memory_by_id` propagates a
`point_is_soft_deleted` error instead of failing open. Failing open would
leak exactly what the contract hides.

**D2 — retiring the second record (WI 3331 decision 2): one proposal,
one keeper, many retirees.** A proposal names `keeper_id`, 1–20
`retire_ids` and an optional `text`.
- *No text* is a duplicate: every retiree gets `superseded_by` = keeper,
  and the keeper is untouched. This is the "A is replaced by existing B"
  that `memory_supersede` could not express.
- *Text* is a merge: a new memory carrying the text supersedes the
  keeper (`supersedes` = keeper), and the keeper and every retiree get
  `superseded_by` = the new memory.

Either way every retired record points at the survivor, so lineage is
kept for all of them. The old options lost it (delete the second
record) or wrote the text twice (supersede both).

**D3 — overlapping clusters (WI 3331 decision 3): re-validate at
acceptance.** Proposals may overlap freely when filed. Acceptance
re-checks that every named record is still live, agent-authored
knowledge. If one is not, the proposal goes `stale`, its note names the
record, and nothing changes. Grouping by keeper at filing time was the
alternative. I rejected it because it moves klams-mind's cluster logic
into klams, and two accepted proposals can still collide when filed
minutes apart. A `stale` proposal is cheap: klams-mind re-files against
the survivor. Bulk resolve applies in the order given, so a reviewer
accepting a cluster's proposals together gets one `accepted` and the
rest `stale`.

**D4 — the merged text is authored by the proposer; the retirement is
attributed to the resolver.** The text is the proposer's wording, so
the new memory's `author_id` is the proposer. `deleted_by_author_id` on
each retired record is the resolver, because that is who made the call.

**D5 — scopes.** `supersede_propose` is `write`, with no ownership check.
Not needing ownership is the point of the tool. `supersede_proposals_list`
is `read`, so a proposer can see its outcomes.
`supersede_proposal_resolve` is `manage`: accepting supersedes another
author's records, which is what `manage` is for. Live grants make that
`claude`, `ghcp` and `claude-duet`. `klams-mind` holds `read,write`, so
it can propose and list but not accept. No config change is needed.

**D6 — MCP only, no REST routes.** Ken's "or Ken in bulk" is met by
`proposal_ids` taking up to 50 per call. No REST consumer exists, so
adding one would be speculative (YAGNI).

**D7 — every argument is a string, a string array or a string enum.**
The untyped payloads behind #850 are the reason `dissent_propose` was
unreachable. The new tools take no `serde_json::Value` at all.

## What shipped

- `klams_core::fetch::memory_by_id` drops soft-deleted knowledge
  (WI 4331). Hermetic tests cover knowledge and facts, and a docker test
  covers the real Qdrant payload.
- Migration `0013_supersede_proposals.sql`; `klams_types::supersede`;
  four `Store` methods (Postgres + composite delegation).
- MCP tools `supersede_propose`, `supersede_proposals_list`,
  `supersede_proposal_resolve`; scope table, descriptors, and a clause
  in the server instructions so an agent that cannot supersede is
  pointed at the propose path.
- `hermetic_supersede_proposals.rs` (11 tests: dedupe, shape refusals,
  both accept shapes, lineage, overlap → stale, rollback on a failed
  mark, reject, not-pending) and docker `mcp_supersede_proposals.rs`
  (the dedupe index, the `manage` gate in the dispatcher, the claim,
  `memory_get` on retired ids).

## Proof from a Claude Code MCP client

I built a throwaway `klams-service` from this branch (`.scratch-agent/058/`)
on port 7958 against the docker test stack, with two identities:
`s058-proposer` (`read,write`) and `s058-curator` (`read,write,manage`).
Then I drove it with `claude -p --mcp-config`, which is a real Claude
Code MCP client, so it loads the advertised schemas the way an agent
session does. In order:
- two `memory_add` calls;
- `supersede_propose` with keeper, retiree and merged text, which
  returned `pending`;
- `supersede_proposal_resolve` accept, as the curator, which returned
  `accepted` with a `result_memory_id`;
- `memory_get` on the retired id, which returned `NOT_FOUND`;
- `memory_get` on the merge, which was live, with `supersedes` = keeper.

Every call succeeded on the first attempt, and the client dropped no
tools. Transcript: `.scratch-agent/058/cc-proof.json`.

## Gates

- `just gate`: green.
- `just test-integration`: green, including the two new docker tests.
  The `mcp_auth` tool-list expectations were updated for the three new
  tools.

## Repaired in passing

- `MemStore::fetch_facts_with_authors` (hermetic test mock) returned
  soft-deleted facts. Postgres has never done that. It now mirrors
  `deleted_at IS NULL` (gate: `hermetic_read_tools`).
- `MemStore` lacked `fetch_events_with_authors` and `mark_superseded`,
  so `memory_get` on a miss and every supersede path were untestable
  hermetically. Both are now implemented honestly (gate: the new tests).

## D8 — restore clears `superseded_by` (overseer call, korg:4401 ruling)

I first reported this as a finding.
`QdrantStore::restore_payload` cleared `deleted_at` and
`deleted_by_author_id` but left `superseded_by`. So a record restored by
`memory_admin_restore`, or by this sprint's own accept-rollback, read as
live while still naming a replacement.

The overseer ruled that restore means "live again", and a live record
must not carry `superseded_by`. Clear it. Lineage is not lost, because
the replacement still carries `supersedes`. **This is a behaviour change
Ken can reverse.** Before 058, `memory_get` and the admin surface showed
a restored superseded record still pointing at its successor. Now they
show it with no pointer.

Tests:
- Admin restore of a superseded record:
  - hermetic: `admin_restore_of_a_retired_record_clears_superseded_by`
  - docker, real Qdrant: `restoring_a_superseded_record_clears_superseded_by`.
    It failed against the pre-fix `qdrant.rs` and passes with the fix.
- Accept rollback: `a_failed_apply_rolls_back_and_returns_to_pending`
  now asserts that the rolled-back record has no `superseded_by`.

Rollback and admin restore share the `restore_payload` primitive. The
docker test pins that primitive. The hermetic test pins that rollback
calls it. A failure cannot be injected into the real Qdrant through MCP.

Gates after the change: `just gate` green, and `just test-integration`
green.

## Deployed 2026-11-09 (PST)

- Version `0.1.58` is live on kubs0. `/healthz` confirms it, with all backends `Ok`; it was `0.1.57`. `klams-service`, `klams-scanner`, `klams-monitor` and `klams-token` each report `--version` 0.1.58.
- Published to the store as `artifacts/klams-*/0.1.58/` via `just publish`, then installed with `just deploy-from-store` and `just restart` (the `deploy-kubs0` skill, declared in `.sprint-deploy`).
- Unit files: unchanged (`git diff 3e8772d..HEAD -- deploy/` is empty), so `install-systemd` was not run.
- kai's `klams-scanner`: left at 0.1.55. This sprint touched no scanner code, so nothing on kai changes behaviour.
- Rollback target: `0.1.57` via `just rollback` (the `.prev` binaries are in place), or any published version via `just deploy-from-store --version`.
- Migrations applied: `0013_supersede_proposals` (table only; no data moved). A binary rollback leaves the table in place, and 0.1.57 ignores it.
- Verified live over raw MCP as `claude`:
  - `tools/list` includes `supersede_propose`, `supersede_proposals_list` and `supersede_proposal_resolve`.
  - `memory_get` on `01a19fd7-a9e8-7613-91f9-40b355f9939b`, the record WI 4331 was filed about, now returns `NOT_FOUND`.
  - `supersede_proposals_list` returns `{"proposals":[]}`, so the migration is applied.
  - `KLAMS_AGENT=claude just health` passed 2, failed 0.
  - Both units are active, with no WARN/ERROR lines since the restart.
- Config changes required: none. `klams-mind` already holds `read,write`; `claude`, `ghcp` and `claude-duet` hold `manage`.
