# Sprint 032 — Breather B: ops drift, docs truth, corpus housekeeping, upgrades

**korg:** proposal 690 · WI #647 (M) #335 (M) #681 (XS) #680 (S) #670 (S) #684 (XS) #688 (S) #648 (M) #334 (M)
**Branch:** `032-breather-b-ops-docs` · **Version:** 0.1.32
**Run mode:** sprint doc first, then build.

## Goal

Sprint 031 made klams' *signals* honest. This one makes its *state* honest:
the hand-maintained things that drifted away from the repo while the
retrieval arc (025–030) had everyone's attention.

Three kinds of lie, in decreasing severity:

1. **The host lies about being rebuildable.** A from-scratch kubs0 rebuild
   following `docs/setup.md` produces a system where the repair tool
   targets a collection that doesn't exist, a duplicate systemd unit
   silently drops auth hot-reload, an undeclared Ollama dependency fails
   its probe forever, and every install starts with a full-admin token
   nobody chose (#647, #670, #335).
2. **The docs lie about the code.** The decay formula is documented as
   exponential and implemented as hyperbolic — every half-life in the
   table is wrong. `architecture.md` still warns readers off ranking that
   024 fixed. Operator recipes contain env vars, config keys, unit names
   and `just` targets that don't exist (#648).
3. **The corpus lies about its own provenance.** Scanned Claude-session
   transcripts sit in Qdrant labelled `AgentProposal`, and a retired
   384-dim collection is still holding ~2 GB as a rollback path that has
   long since proven unnecessary (#688, #684).

Plus the tail: a series contract anchored in a repo nobody may touch
(#680), an OpenAPI spec no loader can parse (#681), and dependencies a
year stale (#334).

**This is not feature work.** No retrieval behavior changes. The eval
suite must come out of this at exactly **21/21, 0 regressions** — the
0.1.30 baseline that 031 preserved.

Sequenced after breather A (031, korg:689) by design: A's PR-branch CI
(#646) is what protects this sprint's wide churn, and A's write-path
refactor (#645) rewrote the files this sprint's dead-code pass deletes
from. Both have landed.

## Ordering

Everything here is independently droppable, so the order is by
entanglement and by risk, not by dependency. **Cut from the bottom** if
timeboxed — #334 is the designated tail.

1. **#647 + #335 together — the ops/dead-code pass.** They overlap on
   three bullets (reattribute-system's default collection, the stale
   duplicate systemd unit, the Ollama/LLM-fallback decision); splitting
   them would mean deciding the same questions twice. **Re-scope #647
   against reality before writing any code** — see "Known shrinkage"
   below.
2. **#670 — raise the decision early, land it late.** It is a *research*
   item needing Ken's sign-off before provisioning changes. Write the
   recommendation in the first pass so it can be answered while the rest
   of the sprint proceeds; implement only after sign-off.
3. **#681, #680 — repo-internal, cheap, self-contained.**
4. **#684, #688 — the live Qdrant ops.** Both are destructive-but-
   authorized production mutations. Backup first; see "Live-ops
   protocol".
5. **#648 — docs truth pass, after 1–4.** Docs should describe the state
   this sprint *ends* in, not the one it started in, so this runs once
   the code and host changes are settled.
6. **#334 — upgrades, last.** Largest blast radius, and the only item
   whose slip costs nothing.

## Scope

### #647 (M) — ops drift bundle

Seven items from the 2026-07-25 deep review (F-3.6 + F-4). **Known
shrinkage — verify before doing:**

- **Backup path** (`ReadWritePaths=/gratch/klams-backup` in the unit vs
  `/ai/klams/backups` in every doc): sprint 028 verified
  `/gratch/klams-backup` is what actually runs. This item is now
  **docs-only** — reconcile the docs and `klams.example.toml` to the live
  value, plus the startup assertion that `backup_dir` is writable.
- **Bare TEI flags**: partially landed. 028 (embedder) and 030
  (reranker) both added explicit flags in compose. Check what remains —
  likely just `--auto-truncate false` being explicit and the comment
  stating the 512-token model limit.

What is expected to survive a fresh look:

- `tools/reattribute-system` `DEFAULT_COLLECTION = "klams_knowledge"` —
  **must now be `knowledge_items_v2`**, not `knowledge_items` as the WI
  says (028 rebuilt the corpus). Run bare today it creates the wrong
  collection and exits 0 reporting zero repairs. Fix the default; drop
  the leftover empty `klams_knowledge` collection on kubs0.
- **Undeclared Ollama dependency** — `[summarization] llm_url` defaults
  to `127.0.0.1:11434`, deployed in no compose file or unit; the probe
  fails every cycle forever. Decided jointly with #335 (see below).
- **Legacy full-scope token** — deferred to #670, which owns the
  decision.
- **Stop the `klams-test-*` compose stack** left running on kubs0; note
  in docs that `tests/docker-compose.test.yml` should be torn down after
  local runs.
- **Delete `deploy/systemd/klams.service`** (stale duplicate; no
  `ExecReload` → a rebuild from it silently loses auth hot-reload). Make
  `deploy_unit_files.rs` recurse so this cannot recur.

**Acceptance**: a from-scratch rebuild following `docs/setup.md` produces
a system where backups write, reload works, the repair tool targets the
right collection, and no surprise admin token exists.

### #335 (M) — dead-code deletion

Full list in korg comment #166. The original three (`KlamsClient::healthz()`,
`clear_cache_for_tests()`, three lint-silencers) plus the review sweep:

- `Embedder::embed_batch` — zero callers repo-wide (3 impls + tests,
  built in 022 for a re-embed never written). **Keep-or-delete is a
  judgment call**: it is the natural hook for a future re-embed, and 028
  did one by other means. If kept, note TEI's default
  `--max-client-batch-size 32` would 413 large batches.
- Knowledge-digest machinery, ~150 lines dead (`qdrant.rs:246-275,
  307-340, 572-640` + the T038 promise in `summarize/mod.rs:11-12`).
  Never wired; delete.
- `summarize/llm.rs` (263 lines) dead in production — this is the same
  Ollama question as #647.3. **Recommendation: delete the LLM fallback
  path** and the `phi3:medium` default rather than deploying Ollama for a
  path nothing has ever exercised; also drop
  `knowledge_stale_days`/`knowledge_cluster_min` (parsed, documented,
  never read). Deploying Ollama would add a model server to the stack to
  serve a feature with no demonstrated demand — YAGNI says delete.
- Small fry: `banner()` ×2, `ApiError::NotImplemented` +
  `ClientError::NotImplemented` (drags a dead code into the OpenAPI
  contract), stacked stale module-docs in `klams-client/src/lib.rs:1-10`,
  the dead `Degraded` health branch (`health.rs:108-109`), the stale
  trait-routing comment at `memory_search.rs:125-129`, the empty
  `tests/fixtures/memories/` placeholder.

**Watch-out**: 031's #645 rewrote the MCP write paths. Re-verify every
line reference in comment #166 against HEAD before deleting — some will
have moved or already be gone.

### #670 (S) — legacy `[auth] bearer_token` posture

A *decision* item, not a change request. Four questions to answer, in
`sprints/032-breather-b-ops-docs/` as a sibling doc:

1. **Is it used?** kubs0's `/etc/klams/klams.toml` has `bearer_token` set
   alongside 14 scoped `[[auth.tokens]]` grants. Establish usage before
   redesigning — if nothing uses it, this is a delete, not a redesign.
   (`just health`/`just verify` default to the literal `dev-token`, so
   the smoke scripts are *not* users of it.)
2. Should it keep granting all four scopes (`read`/`write`/`manage`/
   `admin`), or narrow?
3. Should provisioning stop rendering one by default, in favour of a
   scoped `[[auth.tokens]]` set?
4. Should a grant holding `manage` or `admin` be *required* to declare an
   `agent_name`, so every privileged action is attributable?

**Working recommendation** (to be confirmed with Ken before any
provisioning change): stop rendering a full-scope legacy token by
default; migrate kubs0 to scoped `[[auth.tokens]]` only. If narrowed,
`deploy/config/klams.example.toml` and `docs/auth.md` change together,
and `crates/klams-api/tests/auth_scoped_tokens.rs` pins the new scope set
so it cannot drift silently again. If retired, a migration note — a
deployment with only `bearer_token` and no `[[auth.tokens]]` would lose
all access.

**Blocking on Ken's sign-off.** Nothing here is urgent or externally
exploitable (loopback + tailnet only, constant-time comparison verified).

### #681 (XS) — sprints/002 openapi.yaml unparseable

Fails `yaml.safe_load` at line 272 col 120 ("mapping values are not
allowed here") — an unquoted `:` in a flow mapping or description. Quote
the offending scalar; add a strict-load check to the gate so it cannot
regress. Also decide whether sprint 002's copy is still the live REST
contract or should move somewhere non-sprint-scoped. (AGENTS.md says
don't retrofit 001–012 *layout* — a wrong contract agents act on is a
content problem, not a layout question.)

### #680 (S) — Grafana series contract

`grafana_dashboard_json.rs::every_panel_series_appears_in_handoff_table`
cross-checks `deploy/grafana/klams.json` against a markdown table in
`ansible-k`, which has been inert since 2026-07-05. Worse, the test
self-skips when the file is absent, so it is a silent no-op on any
machine without that checkout.

**Take option 1**: move the table into klams (`deploy/grafana/SERIES.md`),
repoint `handoff_path()`, drop the env-var escape hatch and the skip.
Acceptance: the test *runs* (not skips) from a bare klams checkout, and
the two sprint-027 series are listed. Remove the stale rows from
`ansible-k/specs/klams-integration/klams-grafana.md` once migrated.

### #684 (XS) + #688 (S) — live corpus housekeeping

**#684**: drop the retired 384-dim `knowledge_items` collection, ~2 GB.
Verify `grep collection /etc/klams/klams.toml` → `knowledge_items_v2`
first. After the drop the binary rollback for the 028 model swap is gone
(reverting would mean a full re-embed, not a config flip) — that is the
accepted trade, v2 has been live since 028 with eval 21/21 since 030.

**#688**: one-time Qdrant `set_payload` flipping `source` →`Task` where
`source = AgentProposal` AND `machine` is set AND the author is a scanner
identity. Cosmetic today (029's curated classifier already gates on
`AgentProposal` AND *no* `machine`) but the payload lies about
provenance. **Also verify no current ingest path can recreate the
shape** — 029's notes believed it was pre-028 only; confirm.

#### Live-ops protocol

Both are destructive-but-authorized production mutations. Per the
standing auto-mode boundary:

1. Fresh backup first: `klams-service --run-backup-now`.
2. Verify live config points at `knowledge_items_v2`.
3. Count points before and after; spot-check a sample after #688.
4. Eval must stay exactly **21/21** after both.

### #648 (M) — docs truth pass

Fix content that is *actively wrong*, priority order (things a reader
will act on):

1. **`memory_delete` `author_id`** — docs say optional; verify against
   what 031's #645/#633 work actually landed and make the docs true.
2. **Decay formula** — all three docs say exponential `exp(-λ·age)`; code
   is hyperbolic `1/(1+λ·age)` (`decay.rs:79-84`). Every half-life in
   `usage.md`'s table is wrong (1e-6 → 11.6 days, not 8.0).
3. **`architecture.md` §2j** still documents pre-024 ranking as a known
   limitation, telling readers to distrust ordering that is now correct.
   Add the 021–024 delta content. Also: REST `/memory/search` hardcodes
   `default_rrf()` and ignores `[retrieval]` config (`search.rs:104`) —
   wire it or document it.
4. **Copy-paste breakage** — `KLAMS_AUTH_BEARER_TOKEN` →
   `KLAMS_AUTH__BEARER_TOKEN`; `task_interval = "60s"` →
   `task_interval_seconds`; `journalctl -u klams.service` →
   `klams-service.service`; `just wait-for-stack` doesn't exist; the
   021/022 purge recipes run `just scanner-once` as the wrong user
   against the wrong cursor DB; purge recipes predate host-scoped delete,
   so a hand-run delete without `machine` now wipes ALL hosts.
5. **Nonexistent observables** — `klams_mcp_scope_denied_total`,
   `klams_mcp_calls_total{token_label}`, `klams_tei_requests_total`,
   `klams_decay_config_reload_total` (off by an `s`).
6. **Agent doc gaps** — stale `http://kubs0:7777/mcp` endpoint (should be
   the tailscale-serve HTTPS URL), no error-code table, no parameter
   limits (query ≤1024, top_k ≤50, event window ≤30d), result shape
   (`ScoredMemory`, score = RRF not similarity) undescribed.
7. Smaller items per the audit tables: exit codes, `agent_name` length,
   `listen_addr` claim, Prometheus/Grafana provisioning claims, dashboard
   panel count, broken viewport.md link, stale setup.md tool list.

**Hard boundary**: fix wrong *content* only. Do **not** restructure
`architecture.md`'s delta sections — that is #692's structural pass in
the queued retrospective (korg:695). Coordinate wording with claude-cleo
#634 (cross-repo agent-instructions rewrite) so it lands once.

### #334 (M) — dependency & infra upgrades

axum 0.7→0.8 (+ axum-prometheus lockstep; route syntax `/:id` → `/{id}`),
thiserror 1→2, metrics 0.23→0.24, Qdrant legacy `search_points` →
`query_points`, Prometheus/Grafana image refresh, pin
`rust-toolchain.toml`.

**Two standing cautions:**

1. A **TEI image bump is a model-serving change, twice over** — Qwen3
   embedder *and* bge-reranker-v2-m3 both run on tag `89-1.9`. Any tag
   change re-runs the full eval suite, same gate as a model swap. While
   there, check whether the new TEI finally serves Qwen3-Reranker
   (upstream PRs #886/#730/#835; see the 030 gotcha memory) — if yes, the
   swap is `RERANKER_MODEL_ID` + an eval bake-off, a nice ride-along.
2. **Qdrant image bumps come after #684's v1 drop**, so a storage-format
   upgrade has less to migrate.

## Acceptance

- `just gate` green; docker-gated integration suite green (CI now runs it
  on this branch — 031's #646).
- `just eval` → **21/21, 0 regressions**. Live service is fine; no
  ranking changes are in scope, so the throwaway-service dance isn't
  needed unless #334 bumps a TEI tag.
- `just health` / `just verify` green against the deployed 0.1.32.
- A `docs/setup.md` read-through produces no known-false instruction.
- kubs0: `knowledge_items` (v1) and `klams_knowledge` collections gone;
  no `klams-test-*` stack running; `/healthz` reports 0.1.32.
- Every covered WI either resolved or explicitly deferred with a comment
  saying why.

## Deliberately out of scope

- **#333** (lexical search) — feature-shaped, and eval has been 21/21
  since 030, so the evidence pressure is weak.
- **#406** (future mount-scan), **#632** (oversize-log research, gated on
  data that has been empty since 028 — closure candidate).
- **Architecture restructuring** and **constant re-derivation** — both
  belong to the queued retrospective (korg:695, #692). Any "this constant
  looks off" observation during this sprint gets **filed as a comment on
  #692**, not fixed here.

## Chronicle

_(Filled in as the work happens — decisions, surprises, what actually
shipped vs. what was planned.)_

---

## Live-ops record (#684, #688, #647.2, #647.6)

Run on kubs0, 2026-07-27. Fresh backup first:
`klams-service --run-backup-now` → `OK run_id=01KYGVD912KZ0D17ZR0B208NDP
duration_ms=101255 artifacts=2`, producing
`/gratch/klams-backup/qdrant-2026-07-27.snapshot` (1.1 GB).

### Collections dropped

| Collection | Points | Dim | Why |
|---|---|---|---|
| `knowledge_items` | 221,327 | 384 | #684 — the retired v1 corpus, rollback target since 028 |
| `klams_knowledge` | 0 | 384 | #647.2 — droppings from `reattribute-system` run with its wrong default |

Config verified pointing at `knowledge_items_v2` first. After: only
`knowledge_items_v2` (180,383 pts, 1024-dim, status green) and
`knowledge_items_test` remain; `/healthz` Ok, qdrant Ok, search returns
hits. The binary rollback for the 028 model swap is now gone — reverting
would mean a full re-embed, which is the accepted trade.

Note: `df` on `/ai` did not move (61 G before and after). Qdrant frees
segment files lazily; the space returns without further action. The
"~2 GB reclaimed" in #684 is a projection, not a measurement.

### #688 — the re-source, narrowed

**The WI's rule was wrong for this corpus and would have caused harm.**
It proposed flipping `source` to `Task` wherever `source = AgentProposal`
AND `machine` is set AND the author is a scanner identity. Measured
reality:

- 23 points match `AgentProposal` + `machine`.
- **None** has a scanner author. The WI's premise — that these came from
  the pre-028 `~/.claude/projects/**/*.jsonl` scanner path — is wrong:
  the 13 transcript chunks were written by **klams-mind** through MCP
  `memory_add`, not by `klams-scanner`.
- The other 10 are genuine agent-authored writes (`copilot-claude-opus-4.7`,
  `GitHub Copilot`, `token-master`, `claude`) on real docs and source
  files, which merely happen to carry `machine`.

`Task` outranks `AgentProposal` in `trust_rank` (2 vs 1). Applying the
blanket rule would have silently **promoted those 10 genuine agent
proposals into a higher trust tier** — introducing exactly the
provenance lie the WI exists to remove.

So the fix was narrowed to the 13 chunks whose `file` is under
`~/.claude/projects/` and ends `.jsonl` — the actual bulk transcript
content. `set_payload source = "Task"` on those IDs only.

Counts: `AgentProposal` total 109 → 96; `AgentProposal` + `machine`
23 → 10 (the genuine writes, untouched, as predicted).

**Can it recur?** No. `crates/klams-mcp/src/tools/memory_add.rs` writes
`machine: None` on the knowledge path, so no current ingest path can
produce `AgentProposal` + `machine`. 029's "pre-028 only" belief was
right about the outcome even though it named the wrong ingester.

### Eval gate

`just eval` → **OK — 21/21 queries passed (100%). 0 regression(s), 0
known-open, 0 newly fixed.** Exit 0. Matches the 0.1.30 baseline
exactly, as required.

(Operator note: `just eval` needs `KLAMS_TOKEN`; it defaults to empty
and fails with "retrieval failed / check klams and KLAMS_TOKEN". The
klams-mind `.env` holds a working scoped token.)

## #334 — what was upgraded, and what was deliberately not

### Done

| Thing | From | To |
|---|---|---|
| `axum` | 0.7 | 0.8 |
| `axum-prometheus` | 0.7 | 0.8 (lockstep) |
| `thiserror` | 1 | 2 |
| `metrics` | 0.23 | 0.24 |
| `metrics-exporter-prometheus` | 0.15 | 0.16 (**forced**, see below) |
| `qdrant-client` | 1.12 | 1.18 |
| Qdrant API | `search_points` | `query_points` (2 call sites) |
| `rust-toolchain.toml` | `channel = "stable"` | `channel = "1.96.0"` |

Two things worth recording because neither was in the WI:

**The axum 0.8 path-syntax break does not fail the build.** `cargo
check` passed clean; the router panics at *runtime* on `/:id`. Six
routes needed `/{id}`. A build-green upgrade that panics on first
request is the argument for running the tests, not just compiling.

**The metrics bump silently emptied `/metrics`.** `axum-prometheus`
0.8 pulls `metrics-exporter-prometheus` 0.16 (facade `metrics` 0.24),
while our three crates pinned exporter `0.15` (facade 0.23). Two
`metrics` versions coexisted, so two *separate process-global
registries* existed: `klams_core::metrics` recorded into one, the
installed recorder rendered the other, and exposition came back empty.
`retrieval_metric.rs` caught it. Had it not, the first symptom would
have been every Grafana panel going No Data after deploy. The fix is
aligning the exporter to 0.16 so there is exactly one `metrics` in the
lock file — which is now the invariant worth remembering: **the
exporter version and the `metrics` facade version are one decision, not
two.**

`rust-toolchain.toml` already existed, so the WI's "pin toolchain" read
as done — but it said `channel = "stable"`, which pins nothing. Now a
concrete `1.96.0`. (`rust-version = "1.83"` in Cargo.toml is the MSRV
floor, a different knob, left alone.)

### Deliberately not done

**Container image refreshes.** The WI asks for a Prometheus/Grafana
refresh. Neither container runs on kubs0 — `docker ps` shows only
postgres, qdrant, tei and reranker; Grafana lives on kubsdb, provisioned
out of ansible-k. So a tag bump here is a text edit to something this
host cannot start, cannot scrape, and cannot render. Bumping a
production tag with no way to verify it is worse than leaving it and
saying so. It needs doing where the containers actually run.

**TEI image tag (`89-1.9`).** Unchanged, per the standing caution: it
serves *both* the Qwen3 embedder and bge-reranker-v2-m3, so a tag change
is a model-serving change twice over and carries the same eval gate as a
model swap. Not something to fold into the tail of a debt sprint.

**Qwen3-Reranker check (the 030 ride-along).** Requires the TEI bump
above plus an eval bake-off to be worth anything. Deferred with it.

**Qdrant image (`v1.18.0`).** The client is now on 1.18, matching the
server, so the version skew that motivated part of this item is gone.
A *server* bump is a storage-format change against a live 180k-point
collection; #684's v1 drop (done this sprint) was the prerequisite, so
it is now unblocked — but it deserves its own change with a backup and
an eval run, not the last hour of this one.
