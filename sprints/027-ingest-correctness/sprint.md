# Sprint 027 — Ingest correctness: the 413 family

**Branch:** `027-ingest-correctness` · **Version:** `0.1.27`
**Proposal:** korg:651 · **Work items:** #629, #420, #656, #632 (partial)
**Source of the analysis:** [`docs/reviews/2026-07-25-deep-review.md`](../../docs/reviews/2026-07-25-deep-review.md)
— F-3.1 (error taxonomy), F-3.2 (silent-loss triangle), F-2.7b (oversize
telemetry), F-2.8 (the 512-token ceiling).

## The one root cause

Nothing in klams knows the embedder's real ceiling. TEI runs
`BAAI/bge-small-en-v1.5` with `max_input_length: 512` tokens and
`auto_truncate: false` (measured live via `/info` on kubs0). Every symptom
below follows from that single missing fact:

- The REST cap is **8192 characters** ([`knowledge.rs:31`](../../crates/klams-api/src/handlers/knowledge.rs#L31))
  — roughly 4× the model's real ~2,000-char capacity. The two numbers were
  never reconciled.
- MCP `memory_add` has **no length cap at all**
  ([`memory_add.rs:255-285`](../../crates/klams-mcp/src/tools/memory_add.rs#L255-L285)),
  while `memory_search` in the same crate does enforce `MAX_QUERY_LEN`.
- `StoreError` has no transient/permanent axis
  ([`lib.rs:30-45`](../../crates/klams-store/src/lib.rs#L30-L45)) — `Embedding(String)`
  is one opaque bucket for client-build failure, dim mismatch, parse failure,
  connection refused, and HTTP 413.
- So the retry loop retries **every** non-2xx ×3 and discards the response
  body ([`embeddings.rs:99-123`](../../crates/klams-store/src/embeddings.rs#L99-L123)
  TEI, [`:230-256`](../../crates/klams-store/src/embeddings.rs#L230-L256)
  OpenAI-compat). TEI's 413 body carries `inputs must have less than 512
  tokens` — the one actionable string — and it is thrown away; only
  `HTTP 413` survives.
- Both MCP sites seeing an embedding error attach `EMBEDDING_UNAVAILABLE` +
  `retry_after_seconds: 5` unconditionally, because the information needed to
  do better was destroyed upstream.

Two consequences, both silent:

1. **REST path** — a chunk under 8192 chars but over 512 tokens is accepted
   with 202, the scanner advances its cursor, then the worker's embed fails
   and it drops the job ([`worker.rs:61-68`](../../crates/klams-core/src/worker.rs#L61-L68))
   **without incrementing `writes_failed`** (that counter is only touched in
   HTTP handlers). kai logged ~30k such drops in one 2h window. `/healthz`
   stayed green throughout — TEI's `/health` answers 200 whenever the model is
   loaded; input rejections never touch it.
2. **MCP path** — the write fails outright, and the agent reading
   `EMBEDDING_UNAVAILABLE` + `retry_after_seconds` reasonably concludes the
   embedder is temporarily down and gives up. The knowledge is never written.

The mirror bug rides along: `INTERNAL_ERROR` is the catch-all at 32 call
sites with no retry hint, so a genuinely transient Postgres pool exhaustion
reports as permanently broken. Same missing axis, opposite direction.

## Scope

### 1. #629 — the transient/permanent axis (load-bearing; everything else builds on it)

Add the classification to `StoreError`, classify at the HTTP boundary
capturing **status + response body**, and retry only what is actually
retryable (5xx / connect / timeout). The correct pattern already exists
in-tree at [`scanner/publish.rs:47-68`](../../crates/klams-scanner/src/publish.rs#L47-L68),
which retries 503 only and fails fast on everything else; it never made it
into `embeddings.rs`. Both embedder implementations get the fix — the
OpenAI-compat path has the identical bug and will matter more after 028's
model swap.

Downstream, map honestly: a new `PAYLOAD_TOO_LARGE { limit, submitted }`
carries the numbers the caller needs to split deterministically on the first
try, and `retry_after_seconds` is reserved for genuinely transient
conditions. The mirror direction gets fixed at the same time: transient
backend failures gain a retry hint instead of reporting as permanent.

### 2. #420 — the shared size gate and the missing metric

A **single token-aware gate in `klams-types`**, used by the scanner chunker,
the REST handler, and MCP `memory_add`. It lives there because `klams-types`
is the only crate all three share (the scanner depends on nothing else) and it
already hosts `normalize_chunk_text` for exactly this reason — one definition
of a shared invariant, no drift.

The estimator is a conservative chars-per-token bound with margin, **not**
the in-tree `tiktoken` (`cl100k_base` is the wrong vocabulary for bge's
WordPiece — it would report confidently wrong numbers). It errs toward
over-estimating tokens: the gate must never pass something TEI will reject,
since that is precisely the silent-drop bug. The ceiling is configurable so
028's model swap is a config change rather than a code change.

Plus the worker failure metric, so a dropped write is visible in Grafana at
all. `--auto-truncate` stays **off**: a truncated chunk that looks complete is
worse than a visible drop, and with the gate in place the question is moot.

### 3. #656 — the oversize-write log

When a `memory_add` is rejected for size, record it: `submitted_chars`, token
estimate, the limit in force, author/agent name, timestamp, and **the full
submitted text**. Storing the text is deliberate — it was content destined for
the store anyway, and it is the "what did we lose" corpus. Table mirrors the
`search_miss` pattern ([`migrations/0010_search_miss.sql`](../../migrations/0010_search_miss.sql)),
written fire-and-forget so a log failure never affects the caller's error.
Modest retention cap. Grafana panel for by-agent / by-size counts on the
existing dashboard.

After 028's model upgrade this becomes a rare-event log — which is exactly
what makes it worth keeping, and it is the instrument that decides whether
#632's server-side chunking is ever actually needed.

### 4. #632 — honest error + documented ceiling only

The `memory_add` tool schema states the practical ceiling, and the docs say it
in whatever unit is actually enforced.

## Explicitly out of scope

- **Server-side chunking (#632's other half) is DEFERRED.** Ken's 2026-07-25
  revision moved the GPU model upgrade earlier, into sprint 028 (#655) — with
  an 8k+-token model the ceiling exceeds anything an agent realistically
  writes, which demotes chunking from "the real fix" to optional. #656's data
  decides whether it is ever needed. Do not build it here.
- **Making `McpState` generic over `S: Store`** (#645, F-3.3) — the keystone
  fix that would collapse the two write paths into one policy. It lands in the
  breather sprint, gated behind 025. This sprint therefore installs the size
  gate in both paths deliberately, knowing they unify later.
- **The model swap and corpus reset** — sprint 028.

## Adjacent cleanup (cheap while the error surface is open)

F-3.1 also flags two rotten codes: `INVALID_KIND` has been unreachable since
the 018 flat schema yet is still documented as live in
[`sprints/007-mcp-server/contracts/error-codes.md`](../007-mcp-server/contracts/error-codes.md),
and `SCHEMA_VALIDATION_FAILED` carries both schema violations and semantic
limits across 22 sites. Fix the documentation drift; leave the overload alone
unless it falls out naturally.

## Acceptance

1. A 413 is **never** retried, and never reported with `retry_after_seconds`.
   Hermetic wiremock test `does_not_retry_413` on both embedder paths — the
   5xx-retry counterpart already exists at
   [`embeddings.rs:451`](../../crates/klams-store/src/embeddings.rs#L451), and
   this exact test would have caught the bug pre-merge.
2. An over-limit `memory_add` returns `PAYLOAD_TOO_LARGE` naming the limit and
   the submitted size — enough to split correctly on the first retry, with no
   guessing.
3. No knowledge write is dropped silently: an oversized chunk is rejected at
   the boundary (before the 202 and before the scanner's cursor advances), and
   any worker-level failure increments a metric visible in Grafana.
4. The REST cap and the MCP cap are the same number, and that number derives
   from the configured model's real token budget rather than a hardcoded 8192.
5. An over-limit write produces exactly one oversize-log row carrying the full
   payload; a dashboard panel shows count by agent.
6. A transient backend failure (e.g. pool exhaustion) reports *with* a retry
   hint — the mirror bug is fixed, not just the 413 direction.
7. The ceiling is stated in the `memory_add` tool schema and in the limit
   error itself.

## Sequencing

**027 must land before 028's corpus reset** — the re-scan must not silently
re-drop oversized chunks. That is the whole reason this sprint precedes the
reset in the queue.

## Testing note

`just gate` is not sufficient here. Integration tests don't run on branches
until #646 lands (breather sprint), so the docker-gated tests must be run
locally before merging: `cargo test --workspace -- --ignored` with the test
stack up.

## Cleanup owed

The review-era klams memory `019f9ae4-79c0…` describes the 413 ceiling **as
current behavior**. This sprint obsoletes it, so this sprint supersedes or
deletes it — otherwise klams keeps telling future agents a story that stopped
being true, which is the exact failure mode that produced handoff korg:635.

## Log

### The character-based gate was wrong, and the live model said so

The plan above (and the review's recommendation) called for "a conservative
chars-per-token bound with margin." That was implemented first: ASCII charged at
3 characters per token, non-ASCII at 1:1, with a whitespace-word floor. It passed
a dozen unit tests.

Then it was checked against the live model by binary-searching TEI's real
ceiling per content shape (kubs0, `bge-small-en-v1.5`, 512 tokens,
`auto_truncate: false`):

| shape | chars accepted | chars/token |
|---|---|---|
| punctuation-dense | 525 | 1.03 |
| minified JSON | 788 | 1.55 |
| URLs | 797 | 1.56 |
| random identifiers | 819 | 1.61 |
| many short words | 1020 | 2.00 |
| markdown tables | 1054 | 2.07 |
| Rust code | 1490 | 2.92 |
| English prose | 1691 | 3.32 |
| base64 / hex / one long word | >20000 | >39 |

**A 32× spread.** The `/3` divisor advertised 1530 characters as safe, which is
*over* the real ceiling for URLs, JSON, tables, and punctuation — precisely the
"dense content" #420 named as the cause. The gate would have kept letting through
exactly what it was built to catch.

Nor is this fixable by picking a better constant. A divisor safe for the top of
the table (~1) rejects ordinary 800-character prose chunks and would force the
scanner to split everything, wrecking retrieval; a divisor that leaves prose
alone under-counts punctuation by 3×. **No single ratio is both safe and
useful.**

### What replaced it

TEI exposes `POST /tokenize`, which runs the tokenizer with no model forward
pass — so the exact answer is available and cheap, and costs far less than the
failed embed call it replaces. Design as landed:

- **`Store::check_embed_size`** is the authority, implemented on `CompositeStore`
  via the embedder's new `Embedder::count_tokens`. `TeiEmbedder` answers from
  `/tokenize`; anything else falls back to the estimate.
- **REST and MCP gate on exact counts**, synchronously, before the `202` /
  before the embed.
- **The character estimate survives only where the tokenizer cannot be reached**
  — the scanner (which talks solely to the klams API) and tokenizer-less
  backends. Its docs now state plainly that it is an approximation, with the
  table above, rather than claiming a guarantee it cannot make.
- **`EmbedLimit::certainly_exceeds`** was added for the embedder's preflight,
  because a rejection there is final. It uses a *provable* bound (WordPiece never
  merges across whitespace, so *n* words ⇒ ≥ *n* tokens) instead of the estimate.
  This caught a bug the rework introduced: preflighting on the estimate would
  have refused a 10,000-character base64 blob that the model accepts comfortably
  — a brand-new class of lost write, in the sprint whose entire purpose is
  closing those.
- **The scanner treats a 413 as a permanent per-chunk skip** rather than a
  file-level failure. `publish_chunk` fails fast on non-503, and `scan_root`
  leaves the cursor unadvanced on failure — so a genuinely over-budget chunk
  would have re-offered itself on every scan forever. It is now skipped, counted
  (`chunk_too_large`), and logged; the file's remaining chunks still land.

The calibration is now a test rather than an assumption:
`token_counts_predict_what_tei_accepts` (ignored, needs `TEST_TEI_URL`) asserts
that for all eight shapes above, the gate's verdict matches what the live model
actually does. It passes.

### On `/healthz`

Left deliberately unchanged. TEI's `/health` returns 200 whenever the model is
loaded — input rejections never touch it — so health was never going to catch
this class of failure. The dropped-write counter is the right instrument.

### Pre-existing issues found, filed as WIs rather than fixed here

- **#679** — three `phase4_summarization_pipeline` tests fail in parallel and
  pass with `--test-threads=1`; they truncate the same shared tables
  concurrently. Confirmed pre-existing against a clean worktree at `HEAD`.
  Related to #646: it will surface as a permanently red build the moment
  integration tests start running on branches.
- **#680** — the Grafana series contract that
  `grafana_dashboard_json.rs::every_panel_series_appears_in_handoff_table`
  validates against lives in `~/ansible-k/`, which has been **inert since
  2026-07-05** (`k-homelab` is now the devops owner). The test hard-codes that
  path and silently self-skips when the file is absent. This sprint had to add
  its two new series (`klams_mcp_oversize_writes_total`,
  `klams_writes_failed_total`) to that deprecated repo to get the test to pass
  — **those rows must stay until #680 migrates the table**, or this test breaks.
- **#681** — `sprints/002-safety-and-write-ops/contracts/openapi.yaml` is not
  parseable by a strict YAML loader (fails at line 272 on `HEAD`, before any
  change here).

### Cleanup owed — deliberately deferred to deploy

Memory `019f9ae4-79c0-7480-959d-a0c5a5d4611f` documents the 413 ceiling and ends
with *"supersede this memory when it lands."* It describes **currently deployed**
behaviour, so replacing it before the deploy would tell agents a story that is
not yet true. Supersede it as part of shipping, once 0.1.27 is live on kubs0.

## Deployed 2026-07-26

- Version `0.1.27` live on kubs0 (`/healthz` reports `0.1.27`, status `Ok`,
  postgres/qdrant/embeddings all `Ok`). Units `klams-service` and
  `klams-monitor` active with `NRestarts=0` — no crash loop.
- **Rollback target: `0.1.26`** via `just rollback` (`.prev` binaries in place).
  ⚠️ Binary rollback does **not** undo migration 0012 — crossing back over it
  needs `just restore-from 2026-07-25`.
- **Migrations applied: `0012_oversize_write.sql`** (additive: one new table).
  Data preserved exactly — facts 29→29, events 27→27, authors 43→43,
  search_miss 86→86, Qdrant 221152→221152 after smoke cleanup.
- Backups verified current before deploying: `postgres-2026-07-25.dump` +
  `qdrant-2026-07-25.snapshot` (~22h old on a daily cadence; snapshots growing
  686→747 MB, not shrinking).

### Verified live, beyond `/healthz`

- **#420, the silent-drop regression.** 3999 chars of prose — under the retired
  8192-char cap, over the token ceiling — now returns `413 payload_too_large`
  *before* the `202`, with the numbers attached:
  `"3999 characters (~1137 tokens) exceeds the embedder's 512-token limit;
  split into pieces of at most 1530 characters"`. Pre-027 this was accepted,
  the scanner's cursor advanced, and the worker dropped it silently.
- **Exact token counting is genuinely live**, not the fallback estimate. Two
  independent proofs: the 413 above reports ~1137 tokens where the character
  estimate would have said ~1335; and a 3000-character base64 blob — which the
  estimate refuses at ~1002 tokens but the model accepts at ~77 — was accepted
  with `202` and landed in Qdrant. That second case is the one the estimate
  would have turned into a *new* lost write.
- **#629, the misleading error.** MCP `memory_add` with 2738 characters returns
  `PAYLOAD_TOO_LARGE` and **no `retry_after_seconds`** — the signal that
  previously told agents to wait and retry something that could never succeed.
  Token count reported exactly (~559; the estimate would have said ~915).
- **#656, the oversize log.** That rejection wrote exactly one `oversize_write`
  row: `agent_name=claude`, `submitted_chars=2738`, `estimated_tokens=559`,
  `limit_tokens=512`, `max_chars=1530`, and `length(text)=2738` — the **full**
  payload retained, which is the whole point. `klams_mcp_oversize_writes_total{agent_name="claude"}`
  incremented. That row is left in place deliberately as the demonstration
  #656's acceptance asks for; it ages out via the 90-day prune.
- **The normal path is untouched** — an 800-character prose chunk still gets
  `202` and lands.
- Smoke writes were cleaned up afterwards (2 knowledge chunks deleted, 1
  memory + 1 fact soft-deleted); counts above are post-cleanup.

### Config changes required: none

`[embeddings] max_input_tokens` and `oversize_log_retention_days` both have
defaults matching the deployed model (512) and the intended retention (90), so
`/etc/klams/klams.toml` needed no edit. **Sprint 028 must set both explicitly**
when it swaps the model, along with the scanner's matching `max_input_tokens`
in `/etc/klams/scanner.toml` on kubs0 *and* kai.

### Found during deploy verification

`scripts/verify-mvp.sh` SC-001 is **stale and has been failing regardless of
this sprint** — it posts `{"key","value","subject","source":"verify-mvp.sh"}`,
but the fact API has required `{"type","payload","source"}` with a `Source`
enum since sprint 003. So `just health` and `just verify` cannot pass on any
recent build. Confirmed unrelated to 027 (which touched neither the script nor
`entities.rs`). A hand-built fact write with the current schema succeeds, and
the validator returns correct structured detail, so the write path itself is
healthy. Filed as a WI.
