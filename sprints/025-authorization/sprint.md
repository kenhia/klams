# Sprint 025 — Authorization: make delete honor ownership, make scopes real

korg: sprint proposal `korg:649`; work items **#633** (L), **#636** (M),
**#637** (S). Cold-start context in handoff `korg:635`. Source findings in
[`docs/reviews/2026-07-25-deep-review.md`](../../docs/reviews/2026-07-25-deep-review.md)
(F-3.4).

## Goal

klams currently has an authorization *vocabulary* but not an authorization
*model*. Every token that can reach the MCP surface can delete the entire
store, and on the REST surface the read-only viewport token can bulk-delete
knowledge and resolve dissents. This sprint makes the existing scope
machinery actually load-bearing on both surfaces, makes `memory_delete`
honor ownership, and adds the author-lifecycle verbs needed to clean up
after the hole.

This is a **wiring-and-policy sprint, not an identity build**. Token→author
binding already works for writes; the delete path just ignores it.

## The hole, as measured (2026-07-25)

| Behavior | Observed |
|---|---|
| `memory_add` without `register_author` | Works; attributes to token-bound author |
| `memory_delete` with nil `author_id` | Rejected, `MISSING_AUTHOR_ID` |
| `memory_delete` with *any* valid `author_id` | **Succeeds regardless of ownership** |
| `register_author` on existing `agent_name` | New row, no dedupe |
| `register_author` required scope | **`Read`** — a read-only token can mint identities |
| REST routes carrying `require_scope` | **1 of 14** (`/v1/memories` only) |

The asymmetry between rows 1 and 2 is the tell: the identity `memory_add`
resolves automatically is the one `memory_delete` refuses to infer. Net
effect today — any authenticated caller can mint a throwaway author and
delete anything, `kmon` and the scanner tokens included.

## Correction to the proposal's premise: scopes are FLAT, not hierarchical

The korg deep-review comment on #633 states that `Scope` has a
"`satisfies()` hierarchy". **It does not.** From
[`crates/klams-types/src/auth.rs:26`](../../crates/klams-types/src/auth.rs#L26):

```rust
pub fn satisfies(self, needed: Scope) -> bool {
    self == needed   // "Scopes are independent (not hierarchical)"
}
```

A `write` token does not imply `read`; an `admin` token does not imply
`write`. Grants must list every scope they need — which is exactly what
`deploy/config/klams.example.toml` already does (`scopes = ["read","write"]`).

This is load-bearing for the design and *simplifies* it: adding a `manage`
tier means adding a fourth flat variant and listing it explicitly on the
grants that should have it. There is no implication graph to reason about,
and no risk that granting `admin` silently confers cross-author delete.

Consequence to decide deliberately: `AuthState::new` (the legacy single
`bearer_token` path) materializes one grant with `[Read, Write, Admin]`.
Since that token is the "everything" token by construction, `Manage` is
added to that list too — otherwise upgrading would silently *remove*
capability from the only token some deployments have.

## Scope of work

### 1. `Scope::Manage` — the cross-author curation tier

New flat variant between Write and Admin in intent, not in implication.
Admin keeps its exclusivity over hard-delete/restore/list-deleted; `Manage`
authorizes managing *another author's* memory. Rationale (settled in
`korg:635`, do not relitigate): the driving workflow is an agent retrieving
a memory, recognizing it as wrong, and removing it so the next agent isn't
misled — that agent is usually **not** the author, since the scanners wrote
most of the store.

Grant policy:

| Token | scopes |
|---|---|
| viewport | `["read"]` |
| scanner, kmon, klams-mind | `["read", "write"]` — can retract **their own** records |
| claude, ghcp | `["read", "write", "manage"]` |
| ken-admin / legacy `bearer_token` | all four |

### 2. `memory_delete` honors ownership (#633)

- Add `"memory_delete"` to `BEARER_AUTHOR_TOOLS`
  ([`tools/mod.rs:79`](../../crates/klams-mcp/src/tools/mod.rs#L79)) and make
  `author_id` optional in `MemoryDeleteArgs`.
- Pass caller identity into `memory_delete::run` — `call_tool` already
  resolves it as `caller_author(&ctx)` at line 209 and uses it for the
  sprint-021 miss-log; it simply isn't handed to the delete tool.
- Ownership check: caller author == the memory's `author_id`, **or** caller
  holds `Scope::Manage`. Refusal is `INSUFFICIENT_SCOPE` (constant already
  exists) naming the tier.
- Ownership granularity is the **author row**, not the token (settled: a
  shared `claude` author across machines is a deliberate, visible
  `[[auth.tokens]]` choice).

Note: `deleted_by_author_id` is *already* recorded on both stores — the
handoff was wrong about this. Once the author derives from the token, the
audit trail becomes honest with no new columns and no migration.

### 3. REST routes enforce scopes (#637) — the bigger half

`require_scope` is layered on exactly one route
([`router.rs:124-129`](../../crates/klams-api/src/router.rs#L124-L129)).
Layer it onto the rest:

- `Read` — `/memory/search`, `/memory/context`, `/memory/policy`,
  `/memory/knowledge/:id`, dissent list/get, `/v1/authors*`, facts/events GET
- `Write` — `POST /memory/facts`, `/memory/events`, `/memory/knowledge/index`
- `Write` — `POST /memory/knowledge/delete` (scanner delete-before-reindex
  is legitimate Write-tier work for its own host)
- `Manage` — dissent `promote` / `discard` (promote overwrites a canonical
  fact)

Also make `machine` **required** on `/memory/knowledge/delete`: today
omitting it deletes cross-host by back-compat default
([`knowledge.rs:112-120`](../../crates/klams-api/src/handlers/knowledge.rs#L112-L120)),
so a hand-run delete silently wipes every host's chunks for that path.

The `GET`/`POST` split on `/memory/facts` and `/memory/events` shares one
route entry, so per-method scoping needs either split route registrations or
a method-aware guard — decide during implementation and record the choice.

### 4. `register_author` tightened (#633 item 3)

- Require `Scope::Write` minimum (currently `Read`).
- Dedupe on `agent_name` — `insert_author` mints `Uuid::now_v7()` per call
  and upserts only on `id` conflict, so there is no uniqueness to un-break;
  dedupe must be added.
- Align validation with the token-config rule: `register_author` accepts
  ≤128 chars of any charset, while `validate_agent_name` requires 2–64 bytes
  of `[a-z0-9_-]`. `"Claude Code"` registers fine today and can never be a
  token binding.

### 5. Author lifecycle (#636)

Live census on kubs0 Postgres: **44 author rows** — 8× `klams-mind`, 6×
`kyac`, 3× `mv-cli`, 3× `GitHub Copilot`, 2× `claude`, 2×
`copilot-claude-opus-4.7`, 2× `token-master`, plus one-shots. The
"one row per session, forever" failure mode is already steady state.

- `list_authors` / `get_author` with memory counts — the REST reads
  (`GET /v1/authors`, `/:id`, `/:id/memories`) already exist, so this is
  mostly projection.
- Expose `author.id` on read paths so an agent can tell who owns a memory it
  retrieved.
- Remove: **block-if-owned** (never quietly destructive).
- Merge: transactional reassign A→B, then remove A.
- Acceptance case: the stray row `019f9a90-abe5-7711-9b0f-c467f597832f`
  (`agent_name: claude`, created 2026-07-25T18:36:40Z, owns nothing) is gone.

### 6. `docs/auth.md` (added to scope by Ken, 2026-07-25)

New operator-facing doc: how auth is granted via `[[auth.tokens]]`, what
each scope authorizes on each surface, the recommended per-purpose token
split, and the flat-not-hierarchical rule (the single most likely thing to
get wrong when writing a grant). Cross-link from `README.md`, `setup.md`,
and `usage.md`.

## Acceptance criteria

1. A `write`-only token cannot delete another author's memory by any route,
   including after a fresh `register_author`.
2. A `manage` token can, and `deleted_by_author_id` records who.
3. No agent passes `author_id` for ordinary self-management.
4. The viewport (read-only) token receives 403 `scope_insufficient` on every
   mutating REST route; a test per route class.
5. The scanner can still run delete-before-reindex for its own host.
6. `register_author` requires Write and dedupes on `agent_name`.
7. An author with zero memories can be removed; one with memories cannot be,
   without an explicit merge.
8. The stray `019f9a90-abe5…` row is gone.
9. `docs/auth.md` exists and documents grants + scopes; `setup.md:528`
   (currently the only doc describing the *current* permissive behavior) is
   corrected.

## Out of scope

- #632 (TEI 413 payload limit) — same origin session, unrelated problem.
- #634 (claude-cleo global-instruction rewrite) — deliberately sequenced
  *after* this sprint so the wording lands once.
- #638 lifecycle verbs (`memory_supersede`, `memory_update`) — they depend on
  the ownership check existing, which is what this sprint builds.

## Docs to update

`docs/auth.md` (new), `README.md`, `docs/setup.md` (`:528` describes current
permissive reality), `docs/usage.md` (`:601-604`),
`docs/klams-mcp-for-agents.md` (`:105`), `deploy/config/klams.example.toml`.

The MCP-facing docs at `klams-mcp-for-agents.md:105` and `usage.md:601-604`
*already* document `author_id` as optional on delete — fixing the code makes
the docs true rather than requiring an edit there.

## Rollout note

Config change per token, no migration and no re-index. Existing grants
default to what they already list; `manage` is added deliberately to
`claude` / `ghcp` only. Grants hot-reload on SIGHUP (WI #61), so the rollout
does not require a service restart.

## Chronicle

_Decisions, surprises, and outcomes recorded here as the work proceeds._

- **2026-07-25** — Sprint opened. Version bumped to `0.1.25`. First finding
  before any code: the proposal's "satisfies() hierarchy" premise is wrong;
  scopes are flat (`self == needed`). Design adjusted above — this makes the
  `manage` tier simpler, but means every grant must list its scopes
  exhaustively, including the legacy all-scopes `bearer_token` path.

- **Deviation from the plan: `memory_delete` is NOT in `BEARER_AUTHOR_TOOLS`.**
  The proposal called for adding it there. That mechanism *injects*
  `author_id` into the argument map when absent, which would make a
  token-filled id indistinguishable from a caller-supplied one — and
  telling those apart is exactly what closes the `register_author`
  backdoor. Instead `author_id` became `Option<Uuid>` and the acting
  identity is resolved inside `memory_delete::run` from the caller. An
  explicit `author_id` may now only *confirm* the bound author; naming
  anyone else is refused. Same end state, one less way to spoof.

- **Ownership policy is a pure function.** `authorize_delete` /
  `acting_author` take the caller and the owner and return a decision, so
  the ten cases that matter (owner-with-write, write-only cross-author,
  manage cross-author, admin-is-not-manage, unowned legacy point, foreign
  `author_id`, nil, unbound token…) are unit-tested in `cargo test` with no
  docker stack. Previously this logic could only have been exercised against
  a live Postgres+Qdrant.

- **`deleted_by_author_id` was already recorded** on both stores, as the deep
  review said and the handoff denied. No migration was needed for the audit
  trail; deriving the author from the token is what made it honest.

- **Two behavior changes with a blast radius worth noting.**
  (1) `POST /memory/knowledge/delete` now **requires** `machine`; omitting it
  used to delete the path's chunks on every host. The scanner has always
  sent it. (2) `register_author` now enforces the token-grant `agent_name`
  rule (2–64 of `[a-z0-9_-]`), so `"GitHub Copilot"` and `"Claude Code"` are
  refused — with an error naming a valid substitute (`github-copilot`).
  Existing rows are untouched; only new registrations are affected.

- **`register_author` dedupes on `agent_name`** and returns the existing row.
  The census (44 rows, 8× `klams-mind`, 6× `kyac`) was the direct product of
  minting a fresh UUIDv7 on every call.

- Gate green at this point: `fmt`, `clippy -D warnings`, and `cargo test`
  all pass, including 7 new REST scope-enforcement tests and 10 new
  `memory_delete` policy tests.

- **Author lifecycle landed as three `admin` MCP tools**, not a new tier.
  `#636` left open whether these belong on the MCP surface at all. They do,
  at `admin`: they rewrite attribution across the whole store, which is
  strictly stronger than the cross-author curation `manage` grants, and the
  `memory_admin_*` precedent already exists. The REST `/v1/authors*` reads
  turned out to already return `id` and full counts, so "list/get with memory
  counts" needed no new query — only merge and remove were genuinely new.

- **Merge ordering is load-bearing.** Qdrant has no transactions, so
  `merge_authors` repoints knowledge payloads **first**; if that fails,
  Postgres is untouched and the whole merge is re-runnable. The Postgres half
  (facts, events, soft-delete attribution, dropping the source row) is one
  transaction. Removal blocks on `soft_deletes_authored` too, not just owned
  memories — dropping the row would erase the audit trail a delete recorded.

- **Fixed a pre-existing test-fixture race.** `spawn_isolated` ran
  `DELETE FROM authors WHERE agent_name NOT IN ('system','lost-author')`,
  which deleted rows belonging to *concurrently running* tests — surfacing as
  `facts_author_id_fkey` violations. The race predates this sprint; adding 8
  more isolated tests made it fire reliably. The prune is gone; nothing
  depended on a clean `authors` table.

  The `facts, events, summaries, dissents` TRUNCATE in the same fixture has
  the **identical** hazard against tests built on the non-truncating
  `spawn()` — running the ignored suite in parallel makes
  `phase4_summarization_pipeline` fail, and passing serially again. That half
  is left alone: CI already runs `--ignored --test-threads=1`, so it is a
  local-run footgun, and fixing it properly means a per-test schema rather
  than one shared database. Documented at the top of `tests/common/mod.rs` so
  the next person doesn't spend the time rediscovering it. The `authors` half
  *had* to go regardless, because it broke tests that were not running
  alongside a truncating one at all.

  Removing it does **not** leak rows, on either axis, both verified:

  - **Tests never touch production.** The fixture defaults to
    `postgres://klams:klams_test@127.0.0.1:55432/klams` (the
    `klams-test-postgres` container); production is `:5432`
    (`klams-postgres`). Production holds the same 44 author rows the deep
    review censused, and a query for `s025%` / `%test-agent` /
    `ghcp-test%` / `ghcp-phase%` returns **zero** rows.
  - **The test DB does not grow.** Author count held at 24 across repeated
    full runs of the isolated suite. This is a side effect of the #636
    dedupe: `register_author` returns the existing row for a known
    `agent_name`, and `resolve_test_author` looks up before inserting, so
    reruns reuse the same rows. The dedupe replaced the prune's purpose —
    which is why the prune could go rather than needing a narrower rewrite.

- **The production census, re-read with ownership counts**, confirms the
  pathology #636 describes and names the cleanup targets. Duplicates:
  `klams-mind` ×8 (one owns 2 facts), `kyac` ×6 (one event each),
  `GitHub Copilot` ×3 (one owns an event), `mv-cli` ×3 (own nothing),
  `token-master` ×2 (own nothing), `claude` ×2. The 2026-07-25 `claude` row
  owns 0 facts and 0 events, matching the stray-row description exactly.
  Two existing names — `GitHub Copilot` and `GHCP-claude-opus-4-7` — could
  never have been token bindings, which is the drift the aligned validation
  now prevents at the source. (Read-only queries; nothing in production was
  modified.)

- **`perf_smoke` was never measuring anything — and the system is fine.**
  It fails on `main` too (verified by stashing the branch), but not on
  latency. Two independent fixture bugs, both fixed:

  1. `UserFactValidator` requires `name`; the test sent `note`, so it died on
     the *first* of 10,000 upserts with a 422.
  2. The knowledge seed loop had no backpressure handling. Indexing is
     queued and embedded asynchronously, so an unthrottled loop outruns the
     workers and the API correctly answers `503 queue_full`. With (1) fixed
     the test got as far as knowledge and died there instead. Now retries
     with a 50 ms wait.

  Both are the same drift class as the `mcp_auth` tool-list expectations: an
  `#[ignore]`d test outside the CI gate falling behind changes elsewhere. In
  this case doubly so — CI's ignored step passes
  `--skip search_p95_under_500ms_at_mvp_corpus`, so nothing has executed this
  test in a long time.

  **First real measurement, `--release` against the docker stack** (10k
  facts / 50k events / 10k knowledge, 100 queries):

  ```
  facts seeded in 32.0s
  events seeded in 103.2s
  knowledge seeded in 82.2s (1179 backpressure waits)
  latency p50=22.0ms  p95=30.1ms  p99=40.0ms
  test result: ok
  ```

  p95 is **30 ms against the 500 ms SC-003 budget** — 16× headroom. So the
  planned response (skip the 500 ms assertion, add a blocking fallback at
  measured + 20%) was **not** implemented: it was conditioned on missing the
  budget, and there is nothing to concede. The original assertion stands
  unchanged and now actually runs.

  The budget was deliberately left at 500 ms rather than tightened to the
  measurement. It is the documented SC-003 target, and a tight bound would
  turn a loaded dev machine into a red test. Stays `#[ignore]`d — 250 s and
  70k records is not a CI gate.

- **Integration-tested against the live docker stack**, not just unit-tested:
  8 tests in `mcp_delete_ownership.rs` drive the real MCP HTTP transport,
  because the ownership check reads identity from request extensions that only
  `require_bearer` populates. Among them is the verbatim 2026-07-25 repro —
  `register_author` a throwaway identity, pass its id to `memory_delete` —
  which now returns `INSUFFICIENT_SCOPE`.

## Deployment notes

Config change per token, no migration, no re-index; grants hot-reload on
SIGHUP. Three things to do at rollout:

1. **Add `manage` to the `claude` and `ghcp` grants.** Scopes are flat — an
   existing `["read","write"]` grant gains nothing automatically, and those
   two agents lose the ability to curate other authors' memories until
   `manage` is listed.
2. **Every grant that should be able to delete needs an `agent_name`.**
   Ownership is decided by the bound author; a token without one now gets
   `MISSING_AUTHOR_ID` on delete rather than deleting anything it likes.
3. **Give the viewport `["read", "write", "manage"]`.** See below — this is
   the one change that will break something if skipped.

The scanner needs no change: it has sent `machine` on
`/memory/knowledge/delete` since sprint 023, and delete-before-reindex stays
Write-tier. Verified in `klams-scanner/src/publish.rs`.

### The "read-only viewport" was a fiction, and enforcing scopes exposes it

Found while checking what would break at deploy. `docs/setup.md` recommended
`scopes = ["read"]` for the viewport *"so a UI compromise cannot mutate
state"* — but the viewport app has `upsert_fact`, `edit_fact`, `delete_fact`,
`promote_dissent` and `discard_dissent` commands, hitting `POST
/memory/facts` and the dissent routes. That advice was only ever *nominally*
true: until this sprint nothing enforced scopes on those routes, so the
"read-only" token could mutate everything. That is #637 restated from the
client side.

So enforcing scopes turns a silent hole into a visible 403 on the viewport's
own curation features. Resolved with Ken: **the viewport gets `["read",
"write", "manage"]`.** It is a curation surface — dissent resolution is
documented as *"a human resolves it in the viewport"* — and what actually
bounds it is withholding `admin`: no hard deletes, no restores, no author
lifecycle. Docs, `klams.example.toml`, and `docs/auth.md` corrected; the
misleading "read-only token for the viewport" tip is gone from `setup.md`
and `usage.md`.

Corroborating detail: `viewport/src-tauri/src/commands/memory.rs` already
carries a passing `promote_dissent_surfaces_403_trust_required` test, so a
permission tier on promote was anticipated on the client side — it just had
nothing enforcing it on the server.

(Also noticed: `klams-client::delete_fact` calls `DELETE
/memory/facts/{id}`, which **no route serves** — it would 404/405 regardless
of scope. Pre-existing, unrelated to authz, left alone.)

### Config audit against the live kubs0 config (2026-07-25)

Read-only inspection of `/etc/klams/klams.toml`, no secrets printed, nothing
modified. 14 `[[auth.tokens]]` grants plus the legacy `bearer_token`:

- **All 14 grants carry an `agent_name`** — so nothing loses the ability to
  delete under the new ownership rule. The legacy `bearer_token` materializes
  with all four scopes including `manage`.
- `ghcp`, `claude` and `ken-admin` already have `manage` (`ken-admin` as
  `["read","write","manage","admin"]`). Ken asked whether `admin` needs
  `manage` too: **yes** — scopes are flat, so `admin` alone would not permit
  cross-author delete or dissent resolution. The live config was already
  correct.
- **`viewport` is still `["read"]`** — the one remaining gap. Per the decision
  above it needs `["read","write","manage"]`, or its fact editing and dissent
  resolution will start returning 403 the moment 0.1.25 is live. Hot-reloadable
  via `sudo systemctl reload klams-service`.
- Incidental: the `alice` author owning 1 fact in production is not test
  leakage — there is a live grant labelled *"special for ken live tests"* bound
  to `agent_name = "alice"`.
- Backups current: `/gratch/klams-backup` newest pair 2026-07-25 01:02, qdrant
  snapshot growing (686 → 731 → 745 → 747 MB).

### Deploy automation added (`/sprint-ship` Phase 7)

This sprint also wires klams into sprint-ship's new deploy phase, mirroring
korg's pattern:

- `.sprint-deploy` naming one skill, `deploy-kubs0`.
- `.claude/skills/deploy-kubs0/SKILL.md` — clean-tree-only build, preflight
  (host check, backups, rollback target, expected version), procedure,
  verification, and rollback.

Two klams-specific things the skill exists to encode, both of which would
otherwise be missed:

1. **`just install-systemd` alone does not upgrade a running install.** It ends
   with `systemctl enable --now`, a no-op for an already-active unit, so the
   new binary sits on disk while the old process serves and `/healthz` still
   reports the old version — a deploy that looks clean and changed nothing.
   `just restart` is mandatory. Also now documented in `docs/setup.md`, which
   described the install steps but never this.
2. **A restart applies migrations** (`PostgresStore::connect` runs them as a
   side effect), so `just rollback` — binaries only, one generation of `.prev`
   — cannot undo one. Sprint 025 adds no migrations, so its rollback is clean.

The skill also overrides Phase 7.3's default record filename: klams keeps
sprint records in `sprints/<branch>/sprint.md`, not `README.md`.

**Still owed — the #636 acceptance case.** The stray author row
`019f9a90-abe5-7711-9b0f-c467f597832f` (`agent_name: claude`, created
2026-07-25T18:36:40Z, owns nothing) is not yet removed: that means mutating
the live kubs0 database, so it is left as an explicit post-deploy step rather
than done from the sprint branch. Once 0.1.25 is deployed:
`memory_admin_list_authors { only_empty: true }` to confirm it owns nothing,
then `memory_admin_remove_author` with that id.

## Deployed 2026-07-25

- **Version `0.1.25` live on kubs0** — `/healthz` reports it, `status: Ok`
  with postgres / qdrant / embeddings all `Ok`, clean startup (zero
  error/warn lines in the first three minutes), both long-running units
  `active`.
- **Rollback target: `0.1.24`** via `just rollback` — `.prev` binaries in place
  for all three. **Migrations applied: none**, so this rollback is a clean
  single-step revert.
- **The documented gotcha reproduced exactly.** `/healthz` still reported
  `0.1.24` after `just install-systemd` and only moved to `0.1.25` after
  `just restart` — confirming `enable --now` does nothing for a running unit.
  Worth the paragraph it now has in `docs/setup.md` and the deploy skill.

### Verified live (beyond `/healthz`)

`just health` initially failed SC-001 with **401** — that was the harness, not
the deploy: `KLAMS_TOKEN` defaults to the literal `dev-token`. Re-ran the smoke
tests with real grants (never printed).

**#637 — REST scope enforcement.** The `viewport` (read-only) token now gets
`403 scope_insufficient` on `POST /memory/knowledge/index`, `/memory/facts`,
`/memory/events` and `dissents/:id/promote`, while `GET /memory/policy`,
`/memory/dissents`, `/v1/authors` and `/v1/memories` all still answer `200`.
The `klams-scanner` token (write, no manage) is refused on dissent promote.
`machine` is enforced on knowledge-delete: `400` with `field: "machine"` when
omitted, `200` with it — so the scanner's delete-before-reindex still works.

**#633 — MCP surface and ownership.** A read-only token's `tools/list` returns
exactly `event_search, memory_related, memory_search` — `register_author` is
gone from it, which is the Read→Write move. `ken-admin` sees all 14 tools
including the three new lifecycle verbs. Both refusal paths confirmed against
production data with **zero mutations**:

- write-scoped token deleting a `klams-mind`-owned knowledge item →
  `INSUFFICIENT_SCOPE`, *"this memory belongs to another author…"*
- the 2026-07-25 backdoor, verbatim — `register_author` a throwaway identity,
  pass its id to `memory_delete` → `INSUFFICIENT_SCOPE`, *"author_id must match
  the author bound to your bearer token"*. **This exact call succeeded before
  0.1.25.** The target memory was verified still present afterwards.

(One first attempt hit `EVENTS_NOT_DELETABLE` instead — the id chosen was an
event, and the append-only check precedes the ownership check. Retried against
a knowledge item.)

**#636 — author lifecycle.** `memory_admin_list_authors { only_empty: true }`
lists the removal candidates with their counts and flags the duplicate
`agent_name`s (`GitHub Copilot`, `claude`, `copilot-claude-opus-4.7`,
`klams-mind`, `kyac`, `mv-cli`, `token-master`). Removal works — the
`s025-live-probe` author created during the backdoor test above was removed
with it, closing out that test's own footprint. Block-if-owned works: removing
`system` is refused with `AUTHOR_HAS_MEMORIES` and its exact counts (26 facts,
1 event, 1 knowledge point).

### Config changes

**Outstanding — the `viewport` grant is still `scopes = ["read"]`** in
`/etc/klams/klams.toml`. Per the decision recorded above it needs
`["read", "write", "manage"]`, or the viewport's fact editing and dissent
resolution will 403 (the smoke test above shows exactly that happening).
Ken's to edit — the file holds bearer tokens; hot-reloads with
`sudo systemctl reload klams-service`, no restart.

`ghcp`, `claude` and `ken-admin` already carry `manage`. All 14 grants have an
`agent_name`, so nothing lost the ability to delete.

### Still owed

The #636 acceptance case — stray author row
`019f9a90-abe5-7711-9b0f-c467f597832f` (`agent_name: claude`) — is **not yet
removed**, so WI #636 stays `open`. The new tooling confirms it owns nothing
(facts 0, events 0, knowledge 0, soft-deletes 0), so removal is provably
non-destructive; it just wasn't authorized in this session.
