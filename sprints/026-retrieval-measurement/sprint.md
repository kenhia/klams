# Sprint 026 — Measure retrieval, then stop wasting half of every result page

korg proposal: `korg:650` · work items: **#641** (dedupe + projection),
**#643** (eval suite + instruments) · source: `docs/reviews/2026-07-25-deep-review.md`
(F-2.2, F-2.6, F-2.7)

## Goal

Two halves, deliberately in one sprint so the dedupe lands **measured**:

1. **#641 — query-time content-hash collapse + projection additions.** 44% of
   the corpus is duplicate content; a live 10-result page is 5 duplicate pairs.
   Collapsing at query time doubles effective page capacity immediately, with
   no migration.
2. **#643 — grow klams-mind's eval harness into a suite that measures what
   matters**, and fix the dead instruments (miss-log threshold that can never
   fire, hardcoded `anonymous` caller, no record of what agents actually ask).

Order within the sprint: #641 first — the eval's dedupe invariant and several
of its assertions need `content_hash` in the projection to exist.

## Scope

### #641 — dedupe + projection

Semantics are **pinned by Ken's ruling** (korg WI #641 comment #167 / review
F-2.2); these are not open questions:

- **Key on content only.** Metadata differences (host / file / repo) never keep
  two copies apart. The storage cost of duplicates was never the issue; the
  top-k slots were.
- **The survivor carries the collapsed set** as `copies: [{id, host, file}]` —
  the id list, not merged metadata. Lossless, and lets a caller reach any copy.
- **Top-k scope only.** If a third copy sits outside the fetch window we do not
  hunt for it. (A reverse index by SHA effectively exists — `content_hash` has a
  Qdrant payload index — so completing the set is one filtered lookup if ever
  wanted. Not in v1.)
- **Collapse happens BEFORE fusion**, so freed ranks compact and the released
  slots fill with new content rather than leaving holes.
- **Over-fetch** the Qdrant search (top_k × 2) so a page stays full after
  collapse.

Read paths that must all get the collapse — the review calls this "one seam",
but it is three call sites that each fuse independently:

| Path | Site |
|---|---|
| MCP `memory_search` | `crates/klams-mcp/src/tools/memory_search.rs` |
| REST `/memory/search` | `crates/klams-api/src/handlers/search.rs:102` |
| REST `/memory/context` | `crates/klams-core/src/context.rs:130` |

Projection additions (`PublicMemoryContent::Knowledge`):

- `content_hash` — lets clients and the eval assert the no-duplicates invariant
- `author.id` — ownership reasoning for delete/supersede without a registration
  round-trip (asked for in #631/#636; feeds sprint 025's authz work)
- `heading_path` / `language` / `chunk_index` — lets clients strip breadcrumbs
  and judge fragment-ness

**Discovered during scoping (not in the WI):** those last three are written to
the Qdrant payload (`qdrant.rs:173-181`) but `payload_to_item` never reads them
back, so `KnowledgeItem` does not carry them at all. Projecting them means
extending `KnowledgeItem` + `payload_to_item` first — a bigger change than
"add three fields to the projection" implied.

### #643 — measurement

- Expand `klams-mind` `evals/suites/homelab-retrieval.toml`: the #628 Query A/B
  pair verbatim, 5–10 more curated-beats-bulk cases in *symptom* phrasing, the
  dedupe invariant, an identifier-heavy set (feeds the #333 lexical decision),
  a junk ceiling, the rpidash3 re-merge regression (`korg:635`).
- Wire a klams-side runner so a baseline regression fails visibly.
- Recalibrate `LOW_SCORE_THRESHOLD` (`memory_search.rs:43`) — 0.5 can never fire
  against a bge-small cosine floor of ~0.75; the observed boundary is ~0.78–0.82.
- Add a **search-sample log** (query, caller, top raw scores, hit kinds).
- Fix `record_search("anonymous", …)` (`memory_search.rs:292`) to attribute the
  real caller.

## Out of scope

- **Ingest-time cross-host dedupe** (one point, `machines[]` payload) — that is
  #642, sprint 028, and it rewrites host-scoped delete.
- **Weighted / provenance-aware fusion** — #644, sprint 029. This sprint builds
  the instrument that gates it; it does not change ranking weights.
- The fence-unaware chunker (F-2.3) and the model upgrade (F-2.8).

## Acceptance criteria

1. No two knowledge results in a single page share a `content_hash`, on all
   three read paths.
2. The kpidash repro query (`"kpidash dashboard build commands"`) returns 10
   distinct items, not 5 duplicate pairs.
3. The survivor of a collapse carries `copies: [{id, host, file}]` for every
   suppressed duplicate that was inside the fetch window.
4. `content_hash`, `author.id`, `heading_path`, `language`, `chunk_index` are
   present on knowledge results from every read path.
5. The eval suite fails on a seeded duplicate and on a seeded curated-beats-bulk
   regression — i.e. it can actually detect the failures it claims to guard.
6. The miss log fires on a real weak match; the search-sample log records
   queries with the real caller attributed.
7. `just gate` green; docker-gated integration tests run locally
   (`cargo test --workspace -- --ignored`) before merge — CI does not run them
   on branches until #646 lands.

## Sequencing notes (from the deep-review run notes)

- 026 must land **before** 028 (corpus reset) and **before** 029 (ranking), so
  before/after is measurable. Capture the eval baseline number before 028 wipes.
- 026 runs in parallel with 025 (authz) — different files.
- The klams memory `019f9ae4-687c…` (RRF/duplicates) describes behavior this
  sprint obsoletes; supersede or delete it at ship time.

## Chronicle

_Decisions, surprises, and contract changes get recorded here as the work
proceeds._

- **2026-07-25** — Sprint opened from `korg:650`. Version bumped to `0.1.26`.
  Scoping found the `KnowledgeItem` gap noted above: the three chunk-metadata
  fields are write-only today, so #641's "projection additions" is really a
  store-layer change plus a projection change.

- **2026-07-25 — #641 landed.** Gate green. Notes on what the WI didn't
  predict:

  - **"One seam" was three.** The review describes the collapse as one seam in
    `memory_search`, but `/memory/search` and `/memory/context` fuse
    independently. They do share `StoreHybridAdapter::retrieve`, so the collapse
    went in there once and covers both; MCP `memory_search` needed its own
    (it builds `PublicMemory` directly, not `RankedRow`). The shared logic is
    `klams_core::dedupe::collapse_duplicates`, generic over the row type.

  - **The projection was duplicated four ways.** `KnowledgeItem` → public
    knowledge body was hand-rolled in `klams-mcp::projection`, twice in
    `klams-store::composite`, and once in `memory_admin_list_deleted`; the
    author mapping was hand-rolled at eleven sites. That is *why*
    `heading_path` / `language` / `chunk_index` were stored since 022 and
    projected by nobody — each new field had to be remembered four times.
    Both mappings are now single functions (`PublicMemoryContent::knowledge_from`,
    `PublicAuthorRef::from_record`), so a future field reaches every read path
    or fails to compile.

  - **Found a live bug while adding `host` to the retrieval payload.**
    `matches_filters` reads `host` from the payload, but the vector payload
    never had a `host` key — so `obj.get("host") != Some(want)` was always
    true and **every host-filtered knowledge query returned nothing**. Adding
    `host` (needed anyway for `copies`) fixes it. Pinned by
    `hybrid::tests::host_is_present_in_the_payload_so_a_host_filter_can_match`.
    Not filed separately — it was a one-line fix inside the change that
    exposed it.

  - **Over-fetch is ×2 on MCP, ×3 on the hybrid adapter** (the latter already
    over-fetched for filter pre-pruning, so it was free). ×2 covers the
    measured cross-host *pair*; a chunk present on three hosts can still
    shorten a page. That is the accepted top-k-scope tradeoff from Ken's
    ruling, not an oversight.

  - **`chunk_index` is `u32`**, matching `IndexKnowledge` at ingest rather
    than widening to signed on the way out.

  Tests added: 9 in `klams_core::dedupe` (the collapse contract), 6 in
  `hybrid` (the shared REST/context seam, incl. the host-filter regression),
  6 in `memory_search` (the MCP page), 3 in `klams-types` (wire shape).
  `just gate` green; docker-gated integration tests still to run locally
  before merge.

- **2026-07-25 — #643 landed.** Both halves now in. Notes:

  - **The docker-gated suite caught a real defect `just gate` could not.**
    `memory_search_smoke` failed with `source_rank`s of `[0, 25]` where it
    expected `[0, 1]`: collapse removes entries, so survivors kept their
    *pre-collapse* ranks and the ×2 over-fetch leaked out as an
    uninterpretable gap — also breaking the sprint-017 contiguity
    invariant. Fixed by re-numbering `source_rank` per kind after collapse
    (`renumber_source_ranks`), since the list the caller receives is the
    collapsed one. This is precisely the failure mode the run notes warned
    about: CI does not run `--ignored` on branches until #646.

  - **The eval suite needed an `expect` concept before it could be honest.**
    Several cases this WI mandates are *currently failing* — #628's Query A
    (awaits the ranking sprint), the junk ceiling (awaits the fence-chunker
    fix), the rpidash3 re-merge (awaits #632), and two identifier lookups
    (feed the #333 lexical decision). A suite containing them is
    permanently red and useless as a gate; a suite omitting them measures
    nothing that matters — which is exactly how the old four queries scored
    4/4 while every #628 failure was live. So queries now carry
    `expect = "pass" | "known_open"` plus a `tracking` reference, and the
    gate keys on **regressions**, not raw failures. A `known_open` that
    starts passing is reported loudly as "newly fixed — promote it".

  - **Every eval query was verified against the live corpus, not invented.**
    Mined via MCP on 2026-07-25. Findings worth keeping:
    - `kpidash`/`rpidash3` queries reproduce the duplicate problem exactly:
      a 6-result page came back with two duplicate pairs (same doc on kai
      and kubs0 at ranks 1–2, same README at 3–4).
    - The junk chunk from F-2.3 is real and live: `019f509b-2f45` is
      `"kpidash … > Dashboard build\n\n```bash"` — seven characters of body
      once the breadcrumb is stripped.
    - The rpidash3 regression reproduces: querying that record's *own*
      last-paragraph terms (tailscale/ed25519/cloud-init) returns an
      obsidian Rust-course note at rank 0 and never surfaces the record.
    - #628's Query B still works (gotcha at rank 0, raw 0.785), so the
      Query A failure is ranking, not absence — as #628 claimed.

  - **Threshold set to 0.80, and it is a calibrated constant, not a derived
    one.** Honest only against bge-small. #655's model swap invalidates it;
    `docs/usage.md` now carries the bucket query to re-derive it from the
    sample log, and the constant's doc comment says so.

  - **Baseline not captured as a report.** `just eval` needs `KLAMS_TOKEN`;
    no klams-mind config exists on kubs0 and the service config is not
    readable from Ken's account, so the suite could not be run end-to-end
    here. The before/after was verified through MCP search instead (see the
    deploy record). Setting up a scoped klams-mind token is a small
    follow-up that makes `just eval` usable unattended.

  Tests added this half: 4 in `memory_search` (threshold band, caller
  attribution), 3 more (rank re-numbering), 13 in klams-mind (new check
  types), 8 in klams-mind (expect/known_open gating). Suite grew 4 → 21
  queries. `just gate` green (428 tests); docker-gated suite green (119).

## Deployed 2026-07-26

- Version `0.1.26` live on kubs0 (`/healthz` confirms; was `0.1.25`).
- Rollback target: `0.1.25` via `just rollback` (`.prev` binaries in place).
- **Migrations applied: `0011_search_sample`** (recorded `success = t` in
  `_sqlx_migrations`). It only `CREATE TABLE`s a new table, so a binary
  rollback to 0.1.25 is safe — the old binary simply never writes to it.
  No restore needed to go back.
- Config changes required: **none**.

### Verified live (beyond `/healthz`)

**The dedupe, on the identical query before and after** — `memory_search`
for `"kpidash cross compilation aarch64 toolchain"`, `top_k=10`:

| | 0.1.25 | 0.1.26 |
|---|---|---|
| Results returned | 10 | 10 |
| **Distinct items** | **6** (4 duplicate pairs) | **10** |
| `content_hash` on the wire | absent | present, all 10 distinct |
| `copies` | — | 6 survivors name their collapsed twin |
| `author.id` | absent | present |

The freed slots filled with genuinely new content: ranks 6–9 after the
deploy are chunks that did not appear anywhere in the before-page. That
is the "released slots fill with new content" claim, confirmed in
production rather than in a unit test.

**The instruments.** `search_sample` recorded that same search as
`caller = claude` (not the old hardcoded `anonymous`), `top_kind =
knowledge`, `top_raw_score = 0.879` (the raw cosine, not the RRF 0.016),
and `duplicates_collapsed = 8` — the dedupe's live effect, quantified per
query.

**The miss log is no longer dead.** Before this deploy `search_miss` held
exactly **one** row in production, a `zero_hit` from 2026-07-22 — the
review's central claim, confirmed against live data. A deliberately weak
query after the deploy logged the first `low_score` row klams has ever
recorded (`top_score = 0.639`). Under the old 0.5 threshold it would have
been silently discarded.

**Memory hygiene** (standing caution in the run notes): the review-era
memory `019f9ae4-687c` told agents to "expect ~5 distinct items per 10
results and dedupe by text yourself" — actively wrong and wasteful as of
this deploy. Superseded by `019f9c85-6a0f` (which keeps the still-true
RRF-score and no-provenance-boost parts, and is itself marked to be
superseded when #644 lands) and soft-deleted. The 413-ceiling memory
`019f9ae4-79c0` is untouched — still accurate until #655/#632.

### The eval suite, run live

Ken pointed out the klams-mind token is in that repo's (gitignored)
`.env`, so the suite **did** run against 0.1.26. Baseline captured at
`klams-mind evals/baselines/homelab-retrieval.md`, replacing one that was
five klams sprints stale (2026-07-08 — it recorded `score` values of 0.84,
i.e. raw cosine, which is proof it predates sprint 024's RRF).

**Result: OK — 15/21 queries, 0 regressions, 6 known-open.**

- **`no_duplicates` 3/3** — #641's invariant, asserted against the live
  corpus, not just unit tests.
- The first run reported **2 regressions**; both were investigated and
  neither was one. Both were `source_cited` assertions pinning *which
  file wins* rather than whether the question is answered:
  - `klams.service` — still exists but is a legacy path (live units are
    `deploy/klams-service.service`), and 024's RRF legitimately reordered
    everything. Today's top-5 all answer "kubs0" correctly.
  - `klams.example.toml` for "where does kvllm serve models" — a *klams*
    config that merely mentions kvllm's URL. It was the best answer
    available in July because **kai was not scanned until sprint 023**, so
    the kvllm repo was not in the corpus at all. Today the top-5 is
    kvllm's own justfile / models.toml / helper.py. Retrieval improved;
    the assertion pinned the inferior document.

  Both now assert the answer (`kubs0`, `8000`). Fixed in klams-mind
  `1f60203` with the reasoning recorded inline in the suite.

- Two `known_open` queries passed and were promoted. The more
  interesting one is a genuine **#333 finding**: `find_knowledge_by_content_hash`
  retrieves but `LOW_SCORE_THRESHOLD` does not — both exact identifiers
  in scanned klams source. The difference is that the former appears in
  many chunks surrounded by prose. So the lexical gap is not "identifiers
  never work", it is "identifiers work only when already well represented
  in prose" — precisely the wrong property for looking up a rare symbol.
  Both are kept: one as the regression bar, one as the open evidence.

- The junk ceiling caught the real thing: a 7-character ` ```bash ` body
  in `kpidash/.github/agents/copilot-instructions.md`, breadcrumb
  stripped. F-2.3 confirmed against production.

### Not verified

- `just health` / `just verify` still could not run: they default to
  `KLAMS_TOKEN=dev-token` and the *service* token lives in
  `/etc/klams/klams.toml`, which is not readable from Ken's account.
  `/healthz` passed; the write path was exercised directly via
  authenticated MCP (`memory_add` + `memory_delete` above, both
  succeeded), which covers what SC-001 would have.
- The generated baseline report carries no timestamp or target version of
  its own — worth adding, since a stale baseline is exactly what caused
  the two false regressions above.
