# Sprint 053 — reset-test-stack waits for readiness

Proposal korg:2988, one slice of program korg:2981 (*Low-hanging fruit —
experiment 1*), run as an overseen karc leg (`klams-19748a`) on kubs0.
Covers one work item: **#2283**. XS, no deploy.

## Goal

Make the two-command sequence AGENTS.md documents actually work:

```bash
docker compose -f tests/docker-compose.test.yml up -d
just test-integration
```

It didn't. `scripts/reset-test-stack.sh` probed `$qdrant/readyz` once on
its first line and exited 1 if nothing answered yet, and the hint it
printed on the way out — *"bring the stack up: docker compose … up -d"* —
was the command the operator had just run. The obvious next move is to
re-run `up -d`, which changes nothing and confirms the wrong diagnosis.

## Premise check (start of sprint)

**#2283 — premise holds**, verified live on kubs0 rather than from the
report:

- `up -d` returned after **1s**; `reset-test-stack.sh` then failed
  immediately with exactly the message #2283 quotes.
- `up -d --wait` returned after **16s** with all four services healthy.
  The **reranker** is the laggard, not qdrant.
- `AGENTS.md:95-97` did pair the two commands as consecutive steps with
  nothing in between.

So the warm-up window is ~15s wide and `up -d` returns inside the first
second of it — the failure is not a race that sometimes bites, it is the
documented sequence failing every time from a cold stack.

#2283 also records, correctly, that the recipe's **exit code was never
wrong**: `reset-test-stack.sh` exits 1 and `just` propagates it, and the
exit-0 seen in sprint 048 came from the caller's `| tail -30` pipeline.
Re-confirmed here — every failure path below returns 1. There was
nothing to fix there and this sprint didn't go looking.

## What shipped

### `scripts/reset-test-stack.sh` — a bounded readiness wait

The one-shot probe becomes a poll with a deadline (`TEST_STACK_WAIT_SECS`,
default 60; `0` restores the old no-wait behaviour). It waits on the two
services the sweep actually touches:

- **qdrant** — `curl --max-time 2 "$qdrant/readyz"`.
- **postgres** — `pg_isready -U klams` inside `$TEST_PG_CONTAINER`, but
  **only when that container is running**. The sweep below already
  tolerates an absent postgres (pointing `TEST_QDRANT_HTTP_URL` at a
  standalone qdrant is supported), and waiting 60s in order to then print
  "skipping postgres sweep" would turn a supported case into a stall.

The failure message now distinguishes the two cases it used to conflate,
which is the half of #2283 that made the defect expensive:

- container running → *"IS running, so it is wedged rather than absent"*
  plus `… logs qdrant`.
- no such container → *"the stack is down"* plus `up -d --wait`, and a
  note that `TEST_QDRANT_HTTP_URL` may point somewhere there is no
  qdrant. `TEST_QDRANT_CONTAINER` (default `klams-test-qdrant-1`) exists
  only to tell these two apart.
- postgres the laggard → says so, names the container, points at
  `… logs postgres`.

It also prints one line when it had to wait at all, so a two-second pause
is explained rather than mysterious.

### Docs — `--wait` where the bring-up is documented

`up -d --wait` is docker's own full-stack readiness gate (compose v5.2.0
on kubs0) and covers **TEI and the reranker too**, which the sweep does
not use but the tests do. The script's wait is the belt for anyone who
types the old form or brings the stack up another way; `--wait` is the
better form and now the documented one, in `AGENTS.md` (with the 1s-vs-16s
measurement and why), the `test-integration` recipe comment,
`docs/usage.md`'s recipe table, `tests/docker-compose.test.yml`'s own
header, and `tests/fixtures/backup/README.md`.

### Deliberately not touched

- **`.github/actions/test-stack/action.yml`** — CI already has a working
  readiness gate that probes published ports and asserts each loop rather
  than trusting it. Sprint 031 (#672) chose host probes over the health
  column on purpose, because TEI's cold model load outruns what a
  container healthcheck timeout wants to span. It is not the defect, and
  rewriting the gate that fronts every PR is a behaviour change, not a
  repair.
- **`docs/setup.md`'s restore-drill loop** — polls for healthy already,
  carries its own sprint-032 note, works. *Superseded at ship time*: once
  #3001 added `test-stack-up`, that loop's premise ("no such recipe has
  ever existed") became false — see "Repaired in passing" below.
- **Sprint records** mentioning bare `up -d` — history, not instructions
  (AGENTS.md's historical note).

## Verification

Run on **kubs0**, the host that runs this stack (probe from the machine
that does the work):

| # | Case | Result |
|---|------|--------|
| 1 | Cold stack, `up -d` then the sweep immediately — the reproduction | before: `rc=1` at 1s; after: waited 2s, swept, `rc=0` |
| 2 | Ready stack, sweep | `rc=0`, drops collections as before |
| 3 | Container running, URL answering nowhere | `rc=1`, "wedged rather than absent" |
| 4 | No such container | `rc=1`, "the stack is down", `up -d --wait` hint |
| 5 | Postgres the laggard (bogus running container) | `rc=1`, names postgres |
| 6 | `TEST_STACK_WAIT_SECS=0` against a ready stack | `rc=0`, no wait |

Gates, both read from the command's **own** exit status with the output
captured to a file rather than piped:

- `just gate` — exit 0, no failures, no clippy warnings.
- `just test-integration` — exit 0, **142 tests passed, 0 failed**, with
  `reset-test-stack` visible in the log doing its sweep. This is the path
  the changed script sits directly on.

After the #3001 ruling landed the recipes, the sequence was re-verified in
the form the docs now prescribe, from a cold stack, each exit code read
individually:

```
just test-stack-up      exit 0 after 16s (returns ready, all four healthy)
just test-integration   exit 0 — 142 passed / 0 failed
just test-stack-down    exit 0 — stack gone
```

Note `reset-test-stack` printed **no** waiting line in that run, which is
the correct outcome rather than a missing one: `test-stack-up` returns
ready, so the script's wait had nothing to wait for. Case 1 above is what
proves the wait still works when something else brought the stack up.

`just gate` re-run after the recipe changes: exit 0.

Stack torn down at the end.

### A verification note worth keeping

The first two runs of those gates were `just … 2>&1 | tail -30`, and both
reported exit 0 **from `tail`** — the caller-side pipeline mistake #2283
documents as the thing that produced the false "green" in sprint 048. The
tail also hid every real test count: all it showed was a wall of
`Doc-tests … 0 passed`, which reads exactly like a suite that ran nothing.
Re-run without the pipe, the counts above appeared. The trap is one layer
away from anyone verifying a fix to this script, so: capture to a file,
read the status directly, and check the counts rather than the exit code.

## Repaired in passing — #3001, on the overseer's ruling

Found while fixing #2283 and **filed** first, because the fix changes the
operator-facing recipe surface and that is a decision. The overseer took
the call (close, not Ken-sized, recorded in the program report) and chose
**option 1**, so it was folded into this sprint rather than left for a
later one.

What was wrong: `just compose-up-test` was a bare alias for `compose-up`,
which brings up `deploy/docker-compose.yml` — the **production** stack. So
the recipe whose name ends in `-test` started a second production-shaped
postgres/qdrant/TEI beside the real ones on kubs0, which is the resource
problem sprint 032 (#647) already had to clean up once. Meanwhile there
was no recipe for `tests/docker-compose.test.yml` at all, so every doc
told you to type the raw `docker compose -f …` command. Two sprint-era
quickstarts (007, 008) already called `compose-up-test` "a running test
compose", so the name had misled a reader.

What landed:

- **`test-stack-up`** — `docker compose -f {{test_compose_file}} up -d
  --wait`, and **`test-stack-down`** — the matching `down`. A
  `test_compose_file` variable mirrors the existing `compose_file`.
- **`compose-up-test` retired.** Nothing live called it (checked across
  the whole repo, not just docs: only its own definition, the two
  historical quickstarts, and this sprint record). A tombstone sits in
  `compose-up`'s comment, which is where a reader who typed the old name
  will look.
- **Docs name the recipes**: `AGENTS.md`, `docs/usage.md` (two new table
  rows plus the `test-integration` row), the test compose file's own
  header, `tests/fixtures/backup/README.md`. The raw docker command
  survives once in `AGENTS.md` as prose, for anyone without `just` — and
  once more in the compose file's own header, as explanation of what the
  recipe runs against the healthchecks defined right below it.
- **The two historical quickstarts were left alone** (AGENTS.md's
  historical note; the ruling said so explicitly).

Two self-inflicted problems caught during this, worth recording because
`just`'s doc-comment rule is easy to get wrong: `just --list` shows the
**last** comment line above a recipe, so (a) prepending rationale to
`compose-up`'s comment turned its listed description into the word
"instructions.", and (b) inserting the new recipes directly above
`test-integration` orphaned *its* doc block onto `test-stack-up`. Both
fixed by putting rationale first and the one-line summary last; all four
recipes now read correctly in `just --list`, and `test-integration` gained
a summary line it never had.

### `docs/setup.md`'s restore drill (found at ship time)

Phase 2 of the ship caught staleness **this sprint's own change created**.
The restore drill's step 3 brought the stack up and then polled
`docker compose ps` for `healthy` by hand, under a note reading *"`just
wait-for-stack` was cited here until sprint 032 (#648); no such recipe has
ever existed"*. After #3001 one does, and it waits — so the note was
false and the loop redundant. Step 3 is now `just test-stack-up`, with the
history kept as a comment.

Step 2 deliberately still uses raw `docker compose … down -v`: the drill
wants the volumes destroyed, and `test-stack-down` keeps them. Not a
recipe call to make symmetrical.

Earlier in the sprint this file was listed under "deliberately not
touched" on the grounds that its poll loop worked. That was right when the
recipes did not exist and stopped being right the moment they did.

### `just --list` descriptions for `gate` / `check` / `health`

The same pattern was pre-existing across the justfile, so it was raised
rather than fixed. The overseer ruled it a repair — named, and needing no
decision — so these three are fixed in this branch: `gate` and `health`
had their summary line moved to the **end** of their comment blocks, and
`check` (which explained the alias but never summarised it) gained one.
They now read:

```
check    # Alias for `gate` — the name the kprojects harness uses.
gate     # Constitution pre-commit gate — fail-fast on fmt, clippy, or tests.
health   # Quick liveness probe + light verification round-trip.
```

Scoped to the three named. `identities`, `db-psql`, `backup-size`,
`bench-run`, `eval` and others still list as fragments — same cosmetic
pattern, not named, and widening the sweep would be scope creep rather
than a repair. `just gate` exit 0 afterwards.

## Notes for the record

No test was added under `cargo test`: the changed artifact is a bash
script with no harness in this repo (no bats, no shellcheck recipe), and
the acceptance criterion in #2283 is a two-command sequence against a
live docker stack. Standing one up to assert on it would be a new dev
dependency for an XS fix — YAGNI. The six cases above were run live and
are recorded here instead; case 1 is the before/after.

## Deployed 2026-09-21 — deliberate no-op, nothing was deployed

`.sprint-deploy` names `deploy-kubs0`, so Phase 7 had a declared step. It
was **not run**, on the overseer's ruling in the clearance comment:
*"Nothing on the serve path changed — if the ship's deploy step would
restart the live klams for a justfile/docs/script-only change, record the
no-op instead."*

Both halves of that condition were checked rather than assumed:

- **Nothing on the serve path changed.**
  `git diff --name-only origin/main..HEAD` touches no `crates/`, no
  `migrations/`, no `deploy/`, no `tools/`. The whole diff is `AGENTS.md`,
  `docs/usage.md`, `docs/setup.md`, `justfile`,
  `scripts/reset-test-stack.sh`, `tests/docker-compose.test.yml`,
  `tests/fixtures/backup/README.md`, this record, and `Cargo.toml` /
  `Cargo.lock` — where the only change is `version = "0.1.52"` →
  `"0.1.53"`.
- **`deploy-kubs0` has no self-skipping predicate.** It publishes,
  installs and then restarts, and its own text is explicit that the
  restart "is not optional". So there is no version of running it that
  does not restart `klams-service` and `klams-monitor` — for a release
  whose binaries differ from the running ones only in the version string.

Running it would therefore have taken a healthy service with ~8.8 days of
uptime down and back up to ship no behaviour change at all.

### What this leaves pending, deliberately

**The live fleet stays on `0.1.52` while `main` says `0.1.53`.** Verified
at ship time: `/healthz` reports `0.1.52`, and all four binaries on disk
are `0.1.52` (`service`, `scanner`, `monitor`, `token`).

That gap is the convention working, not breaking: AGENTS.md makes the
version the at-a-glance check for "is the latest sprint deployed", and the
honest answer for 053 is no. The next ordinary klams deploy carries
`0.1.53` and this sprint's version bump with it.

**Whoever runs that deploy should know 053 is in it.** Sprint 052's own
record documents exactly this shape going unnoticed: 051 merged and was
never published or deployed, so store `latest` and the running service
were both still `0.1.50`, and 052's deploy silently carried 051's
klams-monitor change into production for the first time. The difference
here is that this no-op is deliberate and written down — so the next
deploy is a `0.1.52 → 0.1.53` step carrying only dev-tooling and docs, with
no serve-path change to precondition-check.
