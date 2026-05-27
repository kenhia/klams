# klams backlog

Simple backlog for this project. User and agent will add items for future
consideration.

See [plan.md](plan.md) for the phased roadmap and [viewport.md](viewport.md)
for the desktop GUI plan. Items below are deferred or not yet scheduled.

## Agent Instructions

1. New items for the backlog should be created with a section header `## Some feature`
2. Items that have been added to sprint or cut should be removed from this file
   and placed in `specs/planning/backlog-archive.md`
    - If added to a spec/sprint, a markdown-link to the new feature spec should
      be added immediately following the seciton header
    - If the item is moved due to being cut ` **CUT**` should be added to the
      title (i.e. "## Some feature" becomes "## Some feature **CUT**")
## Run full sprint-008 perf baseline against fixed store

Sprint 008 Phase 7 (US5) committed a smoke-sized `perf-baseline.md`
(500 facts / 2,000 knowledge items) to avoid dumping 60k synthetic
rows into the live store while kwi work item #26
(loopback CLOSE_WAIT leak) was open. Once that bug is resolved,
re-run with the full contract corpus:

```bash
just bench-seed -- --facts 10000 --knowledge 50000
just bench-run
```

Commit the refreshed `specs/008-activity-observability/perf-baseline.md`
so the headline numbers match the `100 samples across 10 queries`
target documented in `contracts/bench-harness.md`.

## Per-token author attribution (data-integrity bug)

**REST write paths are losing author identity.** Every fact/event/knowledge
item written via `POST /v1/facts`, `POST /v1/events`, or
`POST /v1/knowledge` is attributed to `SYSTEM_AUTHOR_ID` regardless of
which bearer token issued the write. The worker
([`crates/klams-core/src/worker.rs`](../../crates/klams-core/src/worker.rs))
calls `upsert_fact_v2` / `append_event`, and both hard-code
`SYSTEM_AUTHOR_ID` in
[`crates/klams-store/src/postgres.rs`](../../crates/klams-store/src/postgres.rs)
(lines 85, 105, 510). The `UpsertFact` / `AppendEvent` pipeline structs in
`crates/klams-types/src/pipeline.rs` don't carry an `author_id` field, so
there is no path for the handler to forward a caller identity even if it
had one.

MCP writes are unaffected: `memory_add` / `memory_append_event` require
`author_id` in args, validate it against the `authors` table, and route
to the `_with_author` store variants (postgres.rs:1516, 1554).

**Impact**: per-author listings, author summary aggregates, and the
sprint 008 Activity tab under-report real agents and over-report `system`
for any REST traffic. Data already in the live store is polluted to the
extent REST has been used (today: all 26 facts attributed to `system`).

**Fix outline**:

- Add `agent_name: Option<String>` to `TokenGrantConfig`
  (`crates/klams-types/src/auth.rs`); default to `system` when absent so
  existing deployments stay valid.
- At service startup, resolve each grant's `agent_name` to an `author_id`
  (create the row in `authors` if missing) and cache the mapping.
- Add `author_id: Uuid` to the `UpsertFact`, `AppendEvent`, and
  `IndexKnowledge` pipeline structs.
- In the REST handlers, read the resolved `author_id` from the request
  extension (populated by the auth middleware) and put it on the job.
- Replace the worker's calls to `upsert_fact_v2` / `append_event` with
  the `_with_author` variants, and add an `index_knowledge_with_author`
  that stamps the Qdrant payload.
- Once REST honors caller identity, register a dedicated `klams-bench`
  grant in `/ai/klams/config/klams.toml` so the seeder writes as its own
  author, and replace the payload-filter `just bench-clean` recipe with
  a one-line author-based purge.

Optional follow-up: a one-shot migration that re-attributes existing
`system`-stamped rows to the correct author by replaying their `events`
provenance — only worthwhile if anyone needs to recover the historical
attribution, otherwise let the cutover be the dividing line.

## Phase 6 test harness isolation
`crates/klams-service/tests/mcp_phase6.rs` share a single
`knowledge_items_test` Qdrant collection and a single test Postgres
database. Running them in sequence (`--test-threads=1`) lets earlier
tests' rows skew the counter assertion in
`memory_admin_list_deleted_smoke`. Fix options: (a) per-test
randomized collection name + truncate facts table in setup, (b)
move to `testcontainers`-style per-test ephemeral instances.
Low-priority — the underlying delete→restore round-trip is proven by
the other three tests and the live quickstart §8 walk.

## Multi-vector embeddings (text + code)

Separate embedding spaces for prose vs source code, with per-space retrieval
weighting. From Phase 7 of the original plan.

## Lightweight graph memory

Add a relationship/edge layer over facts and knowledge items for multi-hop
queries.

## Memory diffing and replay

Snapshot memory state and compute diffs over time; replay agent sessions
against historical memory.

## Cross-machine caching

Local cache layer on controller machines to reduce round-trips to `kubs0`.

## Multi-agent coordination memory

Shared scratchpad for agents collaborating on the same task.

## Viewport self-update

Tauri updater integration so the viewport pulls new builds without manual
installer copies.

## Viewport code signing

Sign the Windows installer to silence SmartScreen warnings if they become
disruptive.

## Cloud backup sync for klams

Optional sync of `gratch` backup artifacts to off-site cloud storage. Depends
on the existing gratch backup chain.

## Usefulness-signal decay boost

The Phase 2 decay model uses `last_used_at` + `use_count`, which only tell us
that *something touched* a fact, not whether the consumer found it useful.
Add an explicit "this helped" signal that boosts a fact's effective weight
(or slows its decay) when a human or agent confirms the fact resolved an
issue. Avoid forced voting on every retrieval — favor opt-in feedback paths:

- A viewport "this was useful" action on a fact in a recent search result.
- An MCP tool (e.g. `memory.acknowledge_useful`) the agent can call after a
  successful task resolution citing the fact.
- A controller hook that, on successful task completion, boosts every fact
  read during that task's context window.

Store the boosts as a separate per-fact counter (`useful_count`) plus
`last_useful_at`, with their own contribution term in the scoring formula
so the existing decay math stays clean. Decide whether boosts decay
themselves or persist indefinitely as part of this work.


## Viewport surfaces `source` and computed trust rank

The viewport currently renders fact rows without showing the `source`
field (`User` / `Controller` / `Task` / `AgentProposal`). Trust is
derived from `source` via the `MemoryPolicy` table returned by
`GET /memory/policy`, so a viewer who can't see `source` can't tell
why one row beat another in a contradiction, can't spot a misattributed
write (e.g. ansible-k posting as `User` instead of `Task`), and can't
sort/filter facts by their writer class.

Concrete asks:

1. Add a `source` column (or badge) to the facts list view. Color-code
   by trust tier or include the numeric rank.
2. On the fact detail pane, show both `source` (the literal) and the
   resolved trust rank from `/memory/policy`, with a one-line
   description from the policy entry (the same `description` field
   the policy endpoint returns).
3. Filter affordance: "show facts written by [source...]" multi-select
   so operators can audit "what did ansible push?" or "what did the
   agent propose?" without dropping to SQL.

Touch points: `viewport/src-tauri` memory commands already return the
full `Fact` shape (which includes `source`); the gap is purely on the
SvelteKit render side.

## Viewport surfaces dedupe / decay weights

Related to the source/trust ask above: facts have `confidence`,
`decay_weight`, `use_count`, and `last_used_at` that no viewport
surface currently exposes. These are the inputs to ranking and the
operator's only window into why a fact has drifted down the search
list. Worth at least a tooltip on the detail pane.
