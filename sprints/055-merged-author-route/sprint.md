# Sprint 055 — the author route is one timeline; provisioning respects /etc/klams

Proposal korg:3232, **slice 3 of program korg:3245** (*Low-hanging fruit —
run 3*). Run as an overseen karc leg (`klams-7f42b9`) on kubs0. It covers
two work items, **#3079** and **#3128**, plus **#3267**, which the
overseer folded in during round 2. Version `0.1.55`.

## Goal

- **#3079.** `GET /v1/authors/{id}/memories` should stop serving kind
  sections (facts, then events, then knowledge oldest-first) and serve
  one newest-first timeline. It does that by going through the merge
  `/v1/memories` has had since #54, with `authors = [id]`. The
  overseer's calls (korg:3232 notes): change it **in place**, give an
  old-format cursor a **400** rather than a wrong page, and set **no
  30-day window cap** (all-time).
- **#3128.** `scripts/provision-storage-root.sh` should also refuse when
  `/etc/klams/` holds live config, and say where that config is. It
  should **refuse**, not render into `/etc/klams/`.

## Premise check: both hold

- **#3079 holds.** `list_author_memories_impl` still ran the sectioned
  scroll, and its knowledge page was a Qdrant point-id scroll with no
  `order_by`. The merged machinery (`list_memories_*_page`,
  `take_merged_page`, `encode_merged_cursor`) was unchanged next door.
  I grepped every repo under `~/src` for callers. The only other code
  that names the route is klams-view's `tests/api_contract.rs`, and it
  stubs the route to assert that klams-view does **not** call it.
  `klams-client::list_author_memories` is a passthrough with no callers.
  So the in-place change breaks nobody.
- **#3128 holds, and was worse than filed.** Lines 57–64 checked only
  `$KLAMS_ROOT/config/`. On kubs0, `/etc/klams/` is `0750 klams:klams`
  and ken is not in the group, so a naive `[[ -f /etc/klams/klams.toml ]]`
  is **false** even though the file exists. The guard the brief
  describes would have let the script run on exactly the host the item
  is about. See the decision below.

## What shipped

### #3079 — merged author timeline

- `list_author_memories_impl` (`crates/klams-store/src/composite.rs`) is
  now a thin adapter. It 404s an unknown author, builds a
  `ListMemoriesQuery` with `authors = [id]`, `since = UNIX_EPOCH` and
  `until = now + 1 day`, maps kinds and state, calls
  `list_memories_impl`, and maps the rows back. The window is all-time
  because the per-kind pages are `created_at DESC` from the keyset, and
  knowledge uses the datetime-index `order_by`, so depth costs nothing
  extra.
- The row mapping clears `memory.deleted_at` and
  `memory.deleted_by_author_id`. This route reports deletion *beside*
  the memory (the wire flattens `memory` next to its own `deleted_at`),
  and leaving them set would emit the key twice. The response shape is
  therefore unchanged, and only order and cursor differ.
- **Cursor.** The new `klams_store::is_timeline_cursor` accepts only the
  current `base64("ns:uuid")` form. `decode_merged_cursor` tolerates the
  legacy `section:ns:uuid` form, and keeps doing so for `/v1/memories`.
  If the author route were read that way, an old `k:0:<uuid>` cursor
  would become a keyset before 1970 and silently end the walk. The
  handler now answers any cursor that is not the current form with
  `400 field=cursor` and a message saying to restart from page one.
- Removed the three helpers only the old route used:
  `PostgresStore::list_facts_by_author`, `list_events_by_author`, and
  `QdrantStore::list_knowledge_by_author`.
- Tests:
  - A unit test `timeline_cursor_is_strict`.
  - An integration test `authors_memories_is_newest_first_across_kinds`.
    It writes event → fact → knowledge, walks at `limit=2` across the
    kind boundary, and expects knowledge, fact, event with no
    duplicates. Under the old code it would have led with the fact.
  - An integration test `authors_memories_old_cursor_returns_400`,
    covering the sectioned form and garbage input.
- Docs: a contract-change note in `docs/usage.md` (Author review
  workflow), serving as the changelog, and the route line in
  `docs/architecture.md`.

### #3128 — provisioning guard

- Before anything is created, the script refuses (exit 1, stderr) in
  two cases:
  - `/etc/klams/klams.toml` or `/etc/klams/compose.env` is visible.
  - `/etc/klams/` exists but the invoking user cannot search it. In
    that case it cannot rule live config out.
- **Decision (mine, flagged for ruling): the unsearchable case refuses
  too.** The brief named the two files. Checking only those would be a
  no-op for the ordinary operator on kubs0. A searchable `/etc/klams/`
  holding neither file still proceeds, so a hardened install that got
  only as far as `mkdir` is not blocked.
- Exercised on kubs0:
  - As ken, it refused on the unsearchable dir, and `/ai/klams/config/`
    stayed empty.
  - As root, it refused naming `/etc/klams/klams.toml`.
  - On a scratch copy pointed at a missing dir, it rendered all four
    files (rc 0).
  - With a scratch dir holding `compose.env`, it refused naming the
    file.
- `docs/setup.md`, Provision the root, now has a "fresh host only"
  paragraph.

## Gates

- `just gate` passes.
- `just test-integration` passes: **144 passed, 0 failed**. That
  includes all four `authors_memories_*` tests.

## Surprises

- **The test stack's host ports are in the kernel's ephemeral range.**
  The first `just test-stack-up` failed to bind 127.0.0.1:57070, which
  an outgoing loopback connection (`TIME-WAIT → :7780`) held as its
  source port. The retry reported TEI `Healthy`, but the container ran
  *with no port published*, and the new knowledge-writing test failed
  `EMBEDDING_UNAVAILABLE`. `--force-recreate tei` fixed it for this
  run. I filed it as **#3267**, because the fix was a choice between
  renumbering five ports across CI and the tests, or reserving them
  host-side (a k-homelab sysctl). The overseer ruled for renumbering
  and folded the item into this sprint. See round 2.
- `make_author` is idempotent on agent name. Rows from the failed run
  therefore trailed under the same author on the next run, and the
  timeline test asserts a three-row prefix plus no duplicates rather
  than an exact list.

## Repaired in passing

- `docs/setup.md` step 4 and the paragraph after it still described a
  "32-byte hex operator token … once on stdout". Sprint 050 removed
  tokens. I corrected both to match the script. The gate is n/a, since
  this is docs, but the text now matches what `provision-storage-root.sh`
  prints.

## Round 2 — #3267, the test stack leaves the ephemeral range

The overseer ruled on 3232: option (a), renumber into 61000–65535 as a
contiguous block, and assert the mappings in `test-stack-up`.

- `ss -ltnu` on kubs0 found one listener in 61000–65535, on
  127.0.0.1:61354. I chose **61400–61404**: postgres 61400, qdrant
  REST 61401, qdrant gRPC 61402, tei 61403, reranker 61404.
- Renumbered in:
  - `tests/docker-compose.test.yml`, whose header now records why
  - `justfile`
  - `.github/workflows/ci.yml` and `.github/actions/test-stack/action.yml`
  - `scripts/reset-test-stack.sh`
  - `crates/klams-service/tests/common/mod.rs` and the eight other test
    defaults
  - `docs/setup.md`
- `git grep` for all five old numbers outside the historical sprint
  records now finds nothing.
- **`test-stack-up` asserts every mapping** after `--wait`. It runs
  `docker compose port <svc> <inner>` for each of the five and must
  get `127.0.0.1:<port>`. Otherwise it exits 1, naming the service,
  the port, what it got, and the `ss`/`--force-recreate` commands to
  run.
- Proof:
  - Positive: `test-stack-down` then `test-stack-up` gave rc 0, with
    all five published.
  - Negative: with reranker stopped, the same check reports
    `reranker UNMAPPED (got 'nothing')`.
  - Full `just test-integration`: **144 passed, 0 failed**.

## Round 3 — CI runs the same port check

Overseer rulings on handoff korg:3277:
- #3128's extra refusal (an unsearchable `/etc/klams/`) is **approved**.
- CI starts the stack through `.github/actions/test-stack/action.yml`,
  which never ran the round-2 check. GitHub's Linux runners have the
  same 32768–60999 ephemeral range, so the check had to be one
  implementation with two callers.

What changed:
- The check moved into `scripts/check-test-stack-ports.sh`. It holds
  the table of five mappings, takes an optional compose-file argument,
  prints `mapped:` per service, and on failure exits 1 with the service
  named.
- `just test-stack-up` is back to a plain two-line recipe: `up -d
  --wait`, then the script.
- The action gains a step, **Check test stack port mappings**, which
  runs the script right after `up -d` and before the wait loop.
- The action still does not use `--wait`, deliberately: TEI's model
  load outlasts any healthcheck timeout, which is why the action probes
  from the host. A mapping is fixed when the container starts, so the
  check is valid immediately, and a missing mapping fails in seconds
  instead of after 120 s as "tei unreachable".

Proof, locally on kubs0:
- `actionlint` is not installed, and it lints workflows rather than
  composite actions anyway. The action parses as YAML with four bash
  steps in the intended order, and I read it through.
- The script passes `bash -n`.
- CI-shaped run (`up -d`, then the script at once): rc 0, all five
  mapped.
- Negative, with the reranker stopped: rc 1, naming `reranker` and
  port 61404.
- `just test-stack-up`: rc 0.
- `just test-integration`: 144 passed, 0 failed.
- `just gate` passes.

CI runs the new step for real at ship.

## Follow-ups

None.
