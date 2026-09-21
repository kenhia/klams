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
- **`docs/setup.md`'s restore-drill loop** — it polls for healthy already
  and carries its own sprint-032 note. Working prose, no defect behind it.
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

## Found in passing, filed (needs a decision) — #3001

`just compose-up-test` is an alias for `compose-up`, which brings up
`deploy/docker-compose.yml` — the **production** stack, not the test one.
And there is no recipe at all for `tests/docker-compose.test.yml`: every
doc tells you to type the raw `docker compose -f …` command. So in a repo
whose whole interface is `just`, the recipe whose name ends in `-test`
starts a second production-shaped stack, which is exactly the resource
problem sprint 032 (#647) had to clean up.

Not repaired here: the fix is a choice about the operator-facing recipe
surface — add `test-stack-up`/`down` and rename or retire
`compose-up-test`, or leave the raw command and only fix the name. Both
change what operators type, and a fix that changes an interface someone
else depends on is a decision, not a repair. #3001 names that decision.

Nothing else was repaired in passing; the gate reported no warnings to
adopt.

## Notes for the record

No test was added under `cargo test`: the changed artifact is a bash
script with no harness in this repo (no bats, no shellcheck recipe), and
the acceptance criterion in #2283 is a two-command sequence against a
live docker stack. Standing one up to assert on it would be a new dev
dependency for an XS fix — YAGNI. The six cases above were run live and
are recorded here instead; case 1 is the before/after.
