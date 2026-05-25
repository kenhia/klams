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

## SC-006 perf benchmark (T062, sprint 007)

Validate sprint-007 success criterion SC-006: load a fixture with
≥ 10k facts + 50k knowledge items, run `memory_search` 100×, and
record p95 latency. Attach the result to a follow-up PR or note it in
the next sprint's spec. Do **not** start tuning work if p95 exceeds
1 s — surface the measurement first and let the user decide whether
the actual overshoot is "good enough" for the homelab before any
optimization work begins.

Deferred from sprint 007 ship per user decision (2026-05-25).

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

## `event_search` MCP tool **PRIORITY**

Dedicated filter-based tool for event lookup. `memory_search` is
semantic/embedding-driven over free text — events have no text body,
only `category` + structured `payload`, so they are not surfaced via
`memory_search` (confirmed during sprint 007 quickstart walk).
Proposed shape:

```
event_search {
  author_id?: string,
  category?: string | string[],
  since?: timestamp,
  until?: timestamp,
  payload_match?: { key: value, ... },   // exact-equality on payload fields
  limit?: int (default 50, max 500),
  order?: "desc" | "asc" (default desc)
}
```

Read scope. Pure SQL — no embedding pipeline involved. Pick up
immediately after sprint 007 ships.

## Grafana "MCP author activity" panel: No Data **PRIORITY**

Quickstart §11: counters under `klams_mcp_*` are emitted correctly by
the service (verified via `curl /metrics`) but the Grafana dashboard
panels render "No Data". Likely a Prometheus scrape config drift or a
PromQL/label-name mismatch introduced when the per-author label set
landed in sprint 007. Blocks SC-005. Investigate Prometheus scrape
target + the panel queries in `deploy/grafana/`.

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
