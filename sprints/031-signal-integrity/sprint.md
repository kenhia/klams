# Sprint 031 — Signal integrity: one write path, PR-branch CI, tests that pass on any stack

**korg:** proposal 689 · WI #645 (L) #646 (M) #687 (S) #679 (S) #672 (XS) #682 (XS) · klams-mind #676 (verify only)
**Branch:** `031-signal-integrity` · **Version:** 0.1.31
**Run mode:** sprint doc first, then build.

## Goal

Every quality signal klams has currently lies in at least one way:

- `just gate` and PR CI never run the integration suite, so integration
  breakage is discovered *after* merge (#646).
- The docker-gated suite doesn't pass reliably even when run, because
  three summarization tests race on shared tables (#679) and the shared
  `knowledge_items_test` Qdrant collection silently accumulates seeds
  until a ranking assertion starves (#687).
- The test stack's qdrant container permanently reports `unhealthy`
  because its probe shells out to a `curl` the image doesn't ship (#672).
- `just health` / `just verify` cannot pass against *any* build, because
  `verify-mvp.sh` SC-001 posts an MVP-era fact shape (#682) — the
  post-deploy gate has been trained-to-ignore red for many sprints.
- MCP fact writes bypass the trust/dissent policy REST enforces, and MCP
  knowledge writes bypass normalization, length caps, and the dedupe
  probe — the documented safety property is not held on the surface
  agents actually use (#645).

The sprint's job is to make the instruments honest. No feature work, no
retrieval behavior change: **the eval suite must come out of this at
exactly 21/21, 0 regressions** (the 0.1.30 baseline).

Sequenced before breather B (korg:690): #646's PR-branch CI is what
protects B's wide churn, and #645 rewrites the files B's dead-code pass
(#335) would otherwise delete from, so doing B first would only create
rebase noise.

## Ordering

The order is load-bearing, not cosmetic — the test-stack work comes
first so there is a trustworthy signal to judge the keystone refactor
against.

1. **#672, #687, #679 — make the docker-gated suite deterministic.**
   Until these land, a red integration run tells you nothing about the
   change under test. Landing them first means #645 gets judged by a
   suite that can actually fail for the right reasons.
   Day-one unblock: a one-time `DELETE /collections/knowledge_items_test`
   against the test stack (it recreates on next connect) gives a green
   baseline immediately; the durable fix follows.
2. **#682 — `verify-mvp.sh`.** Independent, XS, and it restores the
   post-deploy gate this sprint's own deploy will need.
3. **#645 — the keystone.** `McpState` generic over `Store`, MCP writes
   through the shared policy core.
4. **#646 — CI.** Depends on #645 (korg edge `646 depends_on 645`): the
   hermetic MCP tests it wants only become writable once `McpState` is
   generic.
5. **#676 verification + eval + ship.**

## Scope

### #672 (XS) — qdrant healthcheck in the test stack

`tests/docker-compose.test.yml:44` probes with `curl`, absent from
`qdrant/qdrant:v1.18.0`. The WI carries the verified replacement (bash
`/dev/tcp` against `/readyz`, single-quoted so `printf` sees literal
`\r\n`) and records why layering `curl` onto the image was rejected.

Ride-along: `.github/workflows/ci.yml:52-90` currently works *around*
this bug with a host-side port-probe loop and a comment explaining the
missing `curl`. Once the healthcheck is honest, that loop can rely on
`docker compose ps` health — decide whether to simplify it or leave the
host probe as belt-and-braces (the host probe also covers TEI's long
model-load, so it probably stays).

**Caution:** the local test stack on kubs0 is a manually-managed fixture
whose named volumes hold a loaded scale fixture (~10k facts / ~20k
knowledge / ~50k events), and `just backup-size` depends on it running.
Do not `down -v` it casually.

### #687 (S) — shared `knowledge_items_test` collection accumulates seeds

Found during 030's docker-gated run:
`phase4_hybrid_retrieval::literal_and_paraphrase_share_results` failed on
unmodified `main` because ~2 weeks of accumulated seeds filled both
queries' top-10 with stale near-duplicates, starving the
semantic-overlap assertion. Not a fusion bug.

Two options from the WI:

1. Drop/recreate `knowledge_items_test` at suite start (justfile recipe
   step or a one-shot in the harness) — two lines.
2. Move ranking-asserting suites to `spawn_isolated`, which already
   creates a per-test ephemeral collection — more honest (a ranking
   assertion should never share a corpus) but touches per-test seeding.

**Decision to make during the sprint.** Preference: option 2 for the
suites that assert on *order*, option 1 as the cheap floor for
everything else — but weigh option 2's seeding cost against how many
suites actually need it. Record the call here when made.

### #679 (S) — summarization tests race on shared tables

Three tests in `crates/klams-service/tests/phase4_summarization_pipeline.rs`
fail in parallel, pass with `--test-threads=1`. Confirmed pre-existing
(verified against a clean worktree at `41a4cea`, not a 027 regression):
each seeds and `TRUNCATE`s the same shared tables via
`crates/klams-service/tests/common/seed.rs`, so whichever truncates last
wipes the others mid-run.

Constraints from the WI:

- Prefer per-test isolation (own schema/database) over global
  serialization — the rest of the ignored suite parallelizes fine (121
  passed serially in 027) and slowing all of it to fix three tests is
  the wrong trade. Serializing just this file is the acceptable fallback.
- A fresh database is *not* sufficient on its own: `seed.rs:55`
  truncates before migrations have created the tables, so an empty DB
  fails with `relation "facts" does not exist`. Whatever isolation is
  chosen must run migrations first.

**Acceptance:** `cargo test --workspace -- --ignored` passes at default
parallelism, repeatedly. Note this also removes the `--test-threads=1`
from CI line 113 — worth doing, since #646 is about to make that job
run on every PR and its wall-clock will matter.

### #682 (XS) — `verify-mvp.sh` SC-001

The script posts `{key, value, subject, source:"verify-mvp.sh"}`. Two
faults against the current API: `source` must be a `Source` enum variant
(`User`|`Controller`|`Task`|`AgentProposal`), and `UpsertFactRequest`
takes `type` + `payload`, not flat `key`/`value`/`subject`. Re-confirmed
during 030's deploy (422 unknown variant). The write path itself is
healthy — a hand-built current-schema request returns 200.

While in there: `just health` defaults `KLAMS_TOKEN` to `dev-token`
(`justfile:17`), so running it unset fails with a confusing 401 that
reads like an auth regression. Fail fast with "set KLAMS_TOKEN" instead.

**Acceptance is live, not unit:** `just health` passes against the
running service on kubs0 with a real token, and `just verify` runs
SC-001..SC-009 without schema errors. (`KLAMS_TOKEN` — klams-mind's
gitignored `.env` has one.)

### #645 (L) — unify MCP/REST write paths

The keystone. Today `klams-mcp` holds a concrete `Arc<CompositeStore>`
(`tools/mod.rs`) while `klams-api` is generic over `trait Store`. Current
count on this branch: **76 concrete `.postgres`/`.qdrant`/`.embedder`
reach-throughs across 13 tool files** (the review counted 42 across 10 —
the surface has grown since).

The divergence is behavioral, not cosmetic:

| concern | REST | MCP |
|---|---|---|
| fact validation (`ValidatorRegistry`, 1,082 lines) | enforced | absent |
| fact writes | `upsert_fact_v2` (trust ranks, dissent divert, versioning) | **v1** — contradicting facts land canonically, no dissent |
| knowledge length / tags / normalization / dedupe-probe / queue | all enforced | **all absent** — `memory_add` has no length cap and embeds synchronously |

Work:

1. `McpState` generic over `S: Store`. **Post-030 watch-out:** the
   struct has grown since the review counted it — `fusion` (029),
   `embed_limit` (027), `reranker` and `rerank_window` (030) must all
   carry through the generic refactor.
2. Route MCP fact writes through `ValidatorRegistry` + `upsert_fact_v2`;
   MCP knowledge writes through the same normalize/limit/dedupe-probe
   core as REST — **a shared function in `klams-core`, not a copy**.
3. Length caps on MCP `memory_add`, token-aware, shared with the
   #632/#420 gate.
4. Move `AuthenticatedAuthor` / `AuthenticatedScopes` out of
   `klams-api` into a core crate — `klams-mcp` currently depends on
   `klams-api` solely for these two types, an inverted dependency.
5. Deduplicate the copy-pasted window validation (`memories.rs:52-68`
   vs `event_search.rs:87-111`, identical strings) and the
   `fuse_in_place` throwaway-`RankedRow` round-trip.

**This is the sprint's only behavioral change** — MCP fact writes gain
the trust/dissent policy. The docker-gated `mcp_lifecycle_verbs` and
`mcp_auth` suites are the safety net: run both **before and after**, so
a diff in their results is attributable.

**Acceptance:** zero concrete store reach-throughs in tool handlers
(enforce with a grep test so it can't regress); an MCP fact write
contradicting a higher-trust canonical fact lands as a dissent, same as
REST; MCP `memory_add` of over-limit text returns the honest permanent
error, not `EMBEDDING_UNAVAILABLE`.

### #646 (M) — CI that runs where it matters

Every docker-gated job in `.github/workflows/ci.yml` is guarded by
`if: github.ref == 'refs/heads/main'` (4 guards, lines 31/49/53/102/116).
PR branches get fmt + clippy + hermetic only. `just gate` mirrors the PR
job, so local pre-commit has the same blind spot. 25 `#[ignore]`s across
the workspace.

Work:

1. Run the docker-compose integration stack on PR branches. It already
   exists and mirrors production TEI config, so the 413 class *is*
   reproducible in CI — nobody wired it.
   **Watch the disk budget**: the main-branch job already carries a
   "free up disk space" prune step (lines 30-46) because the stack is
   tight on a `ubuntu-latest` runner. Turning it on for every PR means
   that pressure applies always — verify a PR run actually completes
   before declaring this done, and be ready to trim the image set.
2. After #645 goes generic: convert the MCP behavioral tests to hermetic
   mock-store tests so they run everywhere. **Correction to the WI's
   framing** — the 13 `#[ignore]`d files in `crates/klams-mcp/tests/` are
   not merely ignored, they are *hollow stubs*: the body of each is a
   comment pointing at a `klams-service` test. Un-ignoring them yields
   nothing; they have to be written. `klams-api`'s contract tests are the
   template (nine files each define their own `impl Store for MockStore`
   — extracting one shared test-support mock is a reasonable ride-along,
   and arguably required before writing 13 more copies).
3. Add the ~15-line hermetic wiremock test `embed_does_not_retry_4xx`
   (the 5xx counterpart exists at `embeddings.rs:451+`) — the test that
   would have caught the 413 retry bug pre-merge.
4. Decide the perf test's fate: `search_p95_under_500ms_at_mvp_corpus`
   is `--skip`ped even on main (line 113). Either run it on main with a
   realistic threshold, or delete the skip theater. **Recommendation:
   delete it** unless a threshold can be defended — a permanently
   skipped test is the same lie this sprint is about.
5. Make `deploy_unit_files.rs` recurse into `deploy/systemd/` (currently
   unlinted, which is how the stale duplicate unit — breather B's #647 —
   survived).

**Acceptance:** a PR that breaks an MCP tool behavior or reintroduces
4xx-retry fails *branch* CI, not main.

### #676 (klams-mind, `resolved`) — verification only

Already shipped: klams-mind PR #7, squash-merged as `cdf0021`, sprint
record `sprints/007-eval-provenance/sprint.md`. **Do not rebuild
anything.** Confirm a fresh `just eval-report` carries run date, klams
version, and suite hash, then move the WI to `done`.

## Non-goals — do not drift

- **No retrieval or ranking behavior change.** The eval must be
  unchanged at 21/21.
- **No restructuring of `docs/architecture.md`'s delta sections** —
  that is the retrospective's structural pass (#692). This sprint fixes
  wrong *content* only where it touches what it changes.
- **Any "this constant looks off" observation gets filed as a comment
  on #692**, not fixed ad hoc.
- Breather B's items (#647 ops drift, #684 v1 drop, #688 transcript
  re-source, #648 docs pass, #334 upgrades, #670 token decision, #335
  dead code) are out of scope. B's #335 in particular will delete dead
  code from the files #645 rewrites — leave it alone.
- Backlog items deliberately not pulled in: #333 (lexical search), #406
  (mount-scan), #632 (research, gated on empty oversize-log data).

## Verification

- `just gate` green.
- **`cargo test --workspace -- --ignored` green at default parallelism**
  (not `--test-threads=1`) against the local test stack — this is both
  #679's acceptance and the precondition for trusting #646.
- `mcp_lifecycle_verbs` and `mcp_auth` run before and after #645, results
  compared.
- `just health` and `just verify` pass against live kubs0 with a real
  `KLAMS_TOKEN` (#682's acceptance).
- `just eval` — **exactly 21/21, 0 regressions**. No throwaway service
  needed; there are no ranking changes, so the live service is fine.
- A deliberately-broken PR (MCP behavior or 4xx retry) fails branch CI
  (#646's acceptance) — worth proving once with a scratch commit.

## Standing cautions

- Until #646 lands, `just gate` ≠ CI: run the docker-gated suite locally
  before merging.
- The reranker container (`klams-reranker`, port 7071) is part of the
  live stack — anything touching compose must keep it. The stage is
  config-gated via `[retrieval] reranker_url` in `/etc/klams/klams.toml`.
- Post-sprint hygiene: run the standing `klams gotcha` search. The
  0.1.30 search-behavior memory needs superseding only if search
  *behavior* changes — this sprint should not change it.

## Log

_Decisions, surprises, and outcomes get appended here as the work
proceeds._

- **2026-07-26** — Branch cut from `main` @ `59423ce` (0.1.30 deployed,
  eval 21/21). Version bumped to 0.1.31. Proposal korg:689 marked
  `active`. Pre-work survey found two deltas from the WI text, both
  recorded above: #645's reach-through count is 76 across 13 files (not
  42 across 10), and #646's "13 ignored MCP test files" are empty stubs
  rather than written-but-skipped tests.
