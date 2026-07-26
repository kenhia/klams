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
