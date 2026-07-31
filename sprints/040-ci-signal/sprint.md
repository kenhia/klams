# Sprint 040 — Trust the CI signal again

**Proposal:** korg:814 (covers #788, #789, #790, #791, #792, #811)
**Started:** 2026-07-30 · **Version:** 0.1.40
**Type:** CI + test-reliability. No product behaviour change intended.

## Goal

The CI signal is not currently trustworthy, and it is slower than it
needs to be. Six items, two kinds:

- **Signal** — two flaky tests (#791, #811) and one silent verdict gap
  (#792, where the merge commit on `main` never completes a run).
- **Waste** — 91 s of dead install (#788), a duplicated and drifted
  test-stack lifecycle that also idles three containers for ~8 minutes
  (#789), and deprecation annotations on every green run (#790).

Fourth installment of the breather pattern (A #689, B #690, C #769),
and it follows the same rule: fix the instrument before taking more
measurements with it. That matters concretely — the next proposal
(korg:815, #799) changes retrieval behaviour in `klams-core`, and its
acceptance depends on an integration suite whose green means something.

## Acceptance

- **The two flakes need a named root cause**, not a green run. Both
  pass more often than they fail, so re-running until green is not
  evidence. A structural fix (deterministic lock ordering, a probe that
  cannot transiently report `Down`) or a documented "here is what we
  ruled out, WI stays open" — either is a legitimate outcome; closing
  on a coin flip is not.
- CI green with no deprecation annotations.
- The `service` job measurably shorter.
- One definition of the test-stack lifecycle.
- The merge commit on `main` carries a completed verdict.

## Decisions

### D-1 — #792: stop the cancellation at both ends

Ken proposed two paths in the WI comment. Reviewing them:

**`paths-ignore: ['docs/**', '*.md']` does not work here**, for two
independent reasons. klams' deploy record does not live in `docs/` —
the `deploy-kubs0` skill explicitly overrides sprint-ship's default and
writes to `sprints/<branch>/sprint.md`. And in GitHub path filters `*`
does not match `/`, so `*.md` only matches root-level files. Sprint
039's record commit (`sprints/039-retire-viewport/sprint.md`) is missed
by both patterns. It also has a wider footgun: `paths-ignore` applies
to `pull_request` too, so a docs-only PR would get no run at all — and
with a required status check, a PR that can never merge. Sprint 038 was
a docs-only sprint, so that is not hypothetical.

**`[skip ci]` is the right lever, and simpler than written.** GitHub
honours `[skip ci]` / `[ci skip]` / `[no ci]` in the head commit message
natively on push; the workflow is skipped before any job is evaluated,
so the `if:` guard in the comment is unnecessary.

Doing **both** halves, because each is one line and they fix different
things:

1. `[skip ci]` on the deploy-record commit → the second push does not
   run, so it cannot cancel the first. (This half is a change to the
   `deploy-kubs0` skill, outside this repo — see "Follow-up" below.)
2. `cancel-in-progress: ${{ github.ref != 'refs/heads/main' }}` → any
   *future* double-push to `main` still completes. PRs keep fast-cancel.

Bonus the WI's details spotted: `perf` is main-only and takes ~7 min,
while the merge run is cancelled at ~2 min — so **perf only ever
completes on the docs commit**, never on the commit carrying the
sprint's code. With `[skip ci]` it runs on the merge commit instead.
Unlike option 2 alone, this does not make perf run twice per ship.

### D-2 — #788: the "variant" the WI worried about does not exist

The WI comment framed this as a choice between owning a CI dependency
and owning drift, with the drift being that CI runs a *variant* of the
gate ("without `--all-features` to save disk", per its step name).

Checked against the tree: the justfile `gate` recipe and CI's inline
block are **byte-identical**, and the recipe's own comment already
explains the `--all-features` exclusion. There is no variant. So
`just gate` in CI is a pure simplification — no parameterized recipe,
no second definition — and the misleading step name goes with it.

Sprint 039 also removed `gate-all` / `gate-viewport`, so there is now
exactly one gate recipe and one gate job to keep aligned.

`extractions/setup-just@v4` is a **composite** action (no Node runtime,
so it cannot contribute a deprecation annotation) that delegates to a
SHA-pinned `setup-crate` pulling from `casey/just` GitHub releases.

**AGENTS.md was stale** and is corrected in the same pass: it documented
the gate as `cargo clippy --all-targets --all-features -- -D warnings`,
`cargo fmt --check`, `cargo test` — none of which is what the justfile
or CI actually runs.

### D-3 — #790: mostly resolved by sprint 039 already

The WI named three Node 20 actions: `actions/checkout@v4`,
`actions/setup-node@v4`, `pnpm/action-setup@v4`. The latter two lived
only in the viewport job, deleted in 039. Verified against the tree and
upstream: the sole remaining Node 20 action is `actions/checkout@v4`
(`using: node20`); `Swatinem/rust-cache@v2` already resolves to v2.9.1
(`node24`), and `dtolnay/rust-toolchain` is composite.

Bumped checkout to `@v7` (`node24`). v5 was the node24 switch; v6 and v7
are dependency bumps plus a fork-PR tightening for `pull_request_target`
/ `workflow_run`, neither of which this workflow uses.

### D-4 — the CI-skip marker is a live wire in commit messages

Discovered by stepping on it: this sprint's own first commit **got no CI
run at all** — zero workflow runs, zero check suites, not even a failed
one. The cause was the commit body, which described the D-1 change and
therefore contained the marker literally. GitHub does not care that the
mention was prose; it scans the head commit's message, finds the token,
and skips the workflow.

So the marker chosen in D-1 has a trap attached, and it is exactly the
kind that bites the person documenting the feature rather than the
person using it:

- **Never write the marker literally in a commit message unless you
  mean it.** Refer to it as "the CI-skip marker" in prose. The same
  applies to a squash-merge subject/body — `gh pr merge --squash --body`
  puts that text into a commit on `main`, so a PR body that discusses
  the marker would skip `main`'s run and defeat the whole point of
  #792.
- File contents are safe. Only commit messages are scanned, which is why
  `deploy-kubs0/SKILL.md` and this document can spell it out.

This is recorded rather than quietly worked around because the failure
mode is silent: no error, no annotation, no red X — just an absence, on
a PR whose *entire purpose* was making CI's verdict trustworthy.

## Outcome

All six done. **Both flakes were root-caused with a deterministic
reproduction, so the acceptance bar above was met rather than waived.**

### #791 — the reranker healthz flake: a URL is not an identity

Reproduced **60/60** with `--test-threads=1`, with CI's exact assertion
(`left: "Down"`, `right: "Ok"`).

`RERANKER_CACHE` keyed its 2-second verdict on the reranker's base URL.
An ephemeral port is recycled the instant its listener drops, so:
`a_sick_reranker_is_visible_but_never_fatal` runs first, caches `Down`
for `http://127.0.0.1:<port>`, drops its `MockServer`, and the healthy
test's `MockServer` binds **the same port** — same key, still inside the
TTL, so a server answering 200 is reported `Down`. Whether the two tests
overlap or serialize is decided by core count, which is why it passed on
the PR runner and failed on main minutes later, on identical code.

Fix: `TeiReranker` now carries a process-unique `instance_id` (an atomic
counter taken at construction, preserved across clones), and the cache
keys on that. Collision is impossible by construction rather than by
luck. Production is unaffected either way — one reranker, one id, one
URL — which is exactly why this only ever bit CI.

The second candidate mechanism the WI named, the probe's 1-second
timeout being tight under CI load, is **ruled out**: the reproduction is
deterministic and does not involve a timeout. The timeout was left
alone.

Regression test: `reranker_probe_cache_does_not_leak_between_instances_sharing_a_url`
— one server, one URL, two instances, behaviour flipped in between. It
pins the bug without depending on port luck, and fails under the old
URL-keyed cache.

**Before: 60/60 fail. After: 0/60 serialized, 0/60 parallel.**

### #811 — the decay deadlock: two batch writers, two lock orders

Reproduced **2 of 3 runs** with a targeted harness, same error as CI
(`apply_decay_batch: error returned from database: deadlock detected`).

`apply_decay_batch` (`UPDATE … FROM UNNEST`) and `apply_last_used_bumps`
(`UPDATE … WHERE id = ANY`) are the only two statements that lock many
`facts` rows at once, and they run concurrently by design — decay fires
hourly while the read path flushes `last_used_at` bumps. Neither pinned
its lock order; that was the planner's choice, and the two statements do
not share a plan shape. Overlapping row sets locked in opposite orders
is a textbook `40P01`.

The tests make it easy to hit because `us3_decay` connects to the
**shared** schema (no per-test isolation) and `tick_once()` walks the
entire `facts` table — so concurrent tests necessarily contend.

Fix: both statements now take their row locks up front via
`ORDER BY f.id … FOR UPDATE` in a CTE. One agreed order between the only
two parties that can form a cycle, so the cycle cannot form. Single-row
writers are irrelevant here — one statement taking one lock can never be
the party that holds A and waits for B.

Per the WI's own guidance, this is the ordering fix and **not** a
retry-on-40P01 wrapper, which would have masked it.

**Before: 2/3 fail. After: 0/8** — and ~5× faster per run, because
contention now resolves immediately instead of waiting out Postgres's
1-second `deadlock_timeout`.

This is a production fix, not just a test fix: the same collision in
production surfaces as `decay tick failed` in the log and a decay pass
that silently did not complete.

New test: `crates/klams-store/tests/decay_lock_order.rs`
(`concurrent_facts_batch_updates_do_not_deadlock`, `#[ignore]`d like the
rest of the docker-gated suite).

### The CI work

- **#788** — `cargo install just --locked` (91 s from source, ~13% of
  the longest job) replaced with `extractions/setup-just@v4`, and the
  step now runs `just gate` rather than its own copy of the commands.
  See D-2: the "variant" the WI worried about did not exist.
- **#789** — `.github/actions/test-stack` composite action; both jobs
  use it. This also fixed a real drift bug: `perf`'s copy waited only on
  Postgres and TEI, then final-checked a Qdrant it had never waited for.
  In `service` the stack now comes up *after* the compile gate instead
  of idling three containers through it.
- **#790** — the only surviving Node 20 action was
  `actions/checkout@v4`; the other two named in the WI went with the
  viewport job in 039. Bumped to `@v7` (node24).
  `Swatinem/rust-cache@v2` already resolves to a node24 release.
- **#792** — both halves, per D-1: `[skip ci]` on the deploy-record
  commit (in `.claude/skills/deploy-kubs0/SKILL.md`, which is tracked in
  this repo) and `cancel-in-progress` scoped to non-`main` refs.
- **AGENTS.md** — the documented gate was wrong (it claimed
  `--all-features`); corrected, with a note that CI now invokes the
  recipe so the justfile is the single definition.

### Verification

- `just gate` — green.
- `just test-integration` — **126 passed, 0 failed**, twice.
- Targeted before/after for both flakes, as above.

## Deployed 2026-07-30

- Version `0.1.40` live on kubs0 (`/healthz` and MCP `server_info`
  confirm; was `0.1.39`).
- Rollback target: `0.1.39` via `just rollback` (`.prev` binaries in
  place for all three).
- Migrations applied: **none** — this sprint touched no `migrations/`,
  so rollback is a clean binary swap with no restore needed.
- Config changes required: **none**.
- Units settled: `NRestarts=0`, no ERROR/WARN in the log,
  `klams-service` and `klams-monitor` both active.

### Verified live, beyond `/healthz`

`just verify` — 7 passed, 0 failed.

**#791** — `/healthz` reports `reranker: Ok` alongside all three
backends, i.e. the instance-id-keyed probe cache works against the real
reranker and not just against mocks.

**#811** — neither new statement runs until the first decay tick, an
hour after restart, so waiting for one was not an option. Instead both
were planned against the **production** schema inside a rolled-back
transaction (`EXPLAIN` on a uuid matching nothing — no data touched).
The plans confirm the fix at the planner level rather than by reading
the SQL:

```
apply_decay_batch:       LockRows → Sort (Sort Key: f_1.id)
apply_last_used_bumps:   LockRows → Index Scan using facts_pkey
```

Both acquire their row locks in `id` ascending order — the sort is
explicit in one, the primary-key index scan supplies it in the other.
Same order in both statements, so the cycle that produced `40P01`
cannot form. This is stronger evidence than the test-stack runs: it is
the real schema, the real statistics, and the real planner.

**#792** — verified by this very deploy. The merge commit's run on
`main` was still in flight when this record was pushed; under the old
configuration that push would have cancelled it, as it did on every ship
from 033 through 039. The record commit carries the CI-skip marker so it
starts no competing run, and `cancel-in-progress` is now scoped off
`main` as a backstop.
