# Sprint 015 — Companion enablement (klams-mind onboarding)

**Branch:** `015-companion-enablement`
**Type:** feature — give klams-mind (the Python/LangChain companion)
what it needs to operate as a first-class, attributable agent.
**Seed:** [roadmap](../planning/roadmap.md) entry 015; counterpart is
klams-mind sprint 004 (semantic contradiction detection).

## Goal

klams-mind can search, add, **propose a dissent**, and page the corpus
using its own identity; its writes appear under its author in the
viewport. Plus the two read-surface ride-alongs (kwi #31, #32).

## Surface map (binding for both repos)

The agent surface is **MCP-only** (`/mcp`): `register_author`,
`memory_add`, `memory_search`, `memory_related`, `event_search`, and
(new, this sprint) `dissent_propose` — all speaking the `PublicMemory`
projection. REST is the controller/operator surface; klams-mind uses
it only for `GET /v1/memories` (bulk paged reads) and `/healthz`.

## Contract: `dissent_propose` (new MCP tool, Write scope)

Today a dissent is only born on the write path, when a lower-trust
write conflicts with the *same* canonical fact. klams-mind detects
contradictions *semantically*, after the fact — it needs to file a
dissent directly.

**Input**
```jsonc
{
  "author_id":  "<uuid from register_author>",     // required
  "fact_id":    "<uuid of the canonical fact>",    // required, must be live
  "proposed_payload": { ... },                     // required — the corrected
                                                   //   payload, same shape as
                                                   //   the fact type's payload
  "reason": "why the proposer believes the fact is wrong",  // required, 1..2000 chars
  "contradicting_memory_id": "<uuid>"              // optional — the memory that
                                                   //   conflicts with fact_id
}
```

**Output** — mirrors the write-path `DissentSubmittedResponse`:
```jsonc
{ "dissent_id": "...", "fact_id": "...", "status": "pending", "deduped": false }
```

**Semantics**
- Proposal lands as `source = AgentProposal` (lowest trust tier) — an
  external proposal never outranks the write path.
- Dedupe reuses the existing pending unique index
  `(fact_id, payload_hash)`: an identical proposal bumps
  `submission_count` / `last_seen_at` and returns `deduped: true`;
  the original `reason` is kept.
- `fact_id` must exist and be live (not soft-deleted) → `NOT_FOUND`.
- Resolution stays human: the viewport `/dissents` page
  promote/discard flow is unchanged; existing orphan trigger applies.

**Storage** — migration `0009_dissent_proposals.sql` adds three
nullable columns to `dissents`: `reason TEXT`,
`contradicting_memory_id UUID`, `author_id UUID REFERENCES authors`.
Write-path dissents leave them NULL (their provenance is the `source`
tier), so nothing existing changes shape. `Dissent` DTO gains the
three fields as `Option`s — additive for viewport/REST readers.

## Scope

1. Migration 0009 + `Dissent` DTO fields + `propose_dissent` store
   path (Postgres) with tests.
2. `dissent_propose` MCP tool (Write scope) + integration test.
3. Token grant for klams-mind: commented `[[auth.tokens]]` sample in
   the example config; live grant is an operator deploy step.
4. kwi #31: viewport detail routes `/facts/[id]`, `/events/[id]`,
   `/knowledge/[id]` — the Tauri commands (`get_fact`, `get_event`,
   `get_knowledge_item`) and `MemoryDetails.svelte` already exist;
   the route pages were simply never created.
5. kwi #32 verification: sprint 009 (T048/T049) already populates
   author knowledge counts on both list and detail paths — confirm
   against the live service and close.
6. Docs: architecture (tool table + delta section), usage, example
   config; roadmap 015 moved out.

## Acceptance

- `just gate` green.
- MCP integration test: propose → appears in `GET /memory/dissents`
  as pending with reason/author; duplicate propose returns
  `deduped: true`; discard/promote flow still works on it.
- Viewport: `/facts/{id}`, `/events/{id}`, `/knowledge/{id}` render
  the detail view (vitest for `hrefFor` targets still passes).
- kwi #31 fixed; #32 verified-and-close; both noted here.

## Deploy notes (operator steps at ship time)

1. Add the klams-mind grant to `/ai/klams/config/klams.toml`:
   ```toml
   [[auth.tokens]]
   token      = "<32-byte hex>"
   scopes     = ["read", "write"]
   agent_name = "klams-mind"
   ```
2. `just db-migrate` (0009) + restart; rebuild/copy the viewport exe
   for the #31 fix.
3. Hand the token to klams-mind (`KLAMS_TOKEN` in its config).

## Chronicle

- (2026-07-06) Sprint opened on top of merged 014 (PR #15). Contract
  designed against the sprint-002 dissent model: reuse the pending
  dedupe index and state machine; add proposal provenance columns
  rather than a parallel table.
- (2026-07-06) **Found + fixed a sprint-013 regression:** the
  `specs/`→`sprints/` rewrite had edited comments inside applied
  migration files (0001, 0002, 0004), breaking sqlx checksum
  validation — any *newly built* binary would refuse to migrate an
  existing database (production was safe only because its deployed
  binary embeds the old bytes). Restored the original file contents;
  migration files are frozen history and must never be touched by
  repo-wide rewrites.
- (2026-07-06) **Test-stack drift fixed:** `tests/docker-compose.test.yml`
  pinned Qdrant v1.12.4 vs the 1.18.0 client (hard incompatibility) —
  repinned to v1.18.0 and recreated the disposable volumes (v1.18
  cannot read v1.12 segment format).
- (2026-07-06) Implementation landed: migration 0009 (nullable
  `reason` / `contradicting_memory_id` / `author_id` on `dissents`),
  `PostgresStore::propose_dissent` (live-fact check + pending-dedupe
  upsert), `dissent_propose` MCP tool (Write scope, registered +
  scope-gated), viewport detail routes for facts/events/knowledge with
  a route-existence vitest guard, klams-mind token-grant sample in the
  example config.
- (2026-07-06) Verification: 3 new MCP integration tests green against
  the compose test stack (provenance recorded; identical payload
  dedupes with original reason kept; NOT_FOUND on missing/soft-deleted
  facts; validation errors; discard flow unchanged); `us2_dissents`
  still green; viewport vitest 45/45 + svelte-check clean.
- (2026-07-06) **Post-PR fix:** `just viewport-build` failed — the
  root `+layout.ts` sets `prerender = true` (static adapter), and
  dynamic `[id]` routes can't be prerendered. Same solution as
  `authors/[id]`: per-route `+page.ts` with
  `prerender = false; ssr = false` (the SPA `index.html` fallback
  serves them at runtime). All three detail routes gained the file;
  cross-compile build green. Note the vitest route guard checks
  `+page.svelte` existence, not buildability — `just viewport-build`
  remains the real gate for new routes.
- (2026-07-06) **kwi #32 verified fixed in production** (read-only
  probe of `GET /v1/authors`): `klams-scanner` reports
  `counts.knowledge = 52794` — the sprint-009 T048/T049 fix covers it;
  close the item. Bonus sighting: klams-mind's sprint-001 live
  round-trip already registered its authors (model
  `gemma-4-31b-it-awq` via kvllm).
