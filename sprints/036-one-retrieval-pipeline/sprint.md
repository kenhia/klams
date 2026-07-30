# Sprint 036 — One retrieval pipeline

**Proposal:** korg:770 · **Covers:** #730 (L), #731 (M) · **Branch:** `036-one-retrieval-pipeline`

The read-side completion of 031's write-path unification, straight from the
033 retrospective's divergence map (#692).

## Goal

1. **#730 — Unify REST and MCP retrieval.** MCP `memory_search` runs the real
   pipeline (curated stratum, 3-tier provenance weights, boost gate, dedupe,
   rerank, weighted RRF, miss+sample logs); REST `/memory/search` runs a
   divergent `StoreHybridAdapter` path (no curated stratum, no rerank,
   author-blind two-tier weights that mis-weight klams-mind extracts at 2.0
   instead of 1.5, hardcoded `default_rrf()` ignoring `[retrieval] fusion`);
   `memory_related` is bare ANN. Lift `memory_search::run`'s body into a
   klams-core function over `(store, params, RetrievalConfig)`; memory_search.rs
   becomes a validation+envelope shell; REST calls the same core and projects
   `ScoredMemory → SearchHit`.
2. **#731 — Reranker observability.** Surface the reranker in `/healthz`
   (visible but non-fatal — a sick reranker must not flip overall status),
   give `TeiReranker::health()` a caller, make the live-rerank integration
   test runnable (`tests/docker-compose.test.yml` gains a CPU reranker
   service; `just test-integration` wires `TEST_RERANKER_URL`), and settle
   the bench-speaks-REST question.

## Known blockers (mapped by #730)

- `klams-mcp/src/projection.rs` moves to klams-core/klams-types (klams-api
  must not depend on klams-mcp).
- A neutral error type both surfaces map (ErrorEnvelope vs ApiError).
- `ApiState` gains fusion/reranker/rerank_window — 12 construction sites
  (1 main.rs, 11 test files).

## Contract decisions (as made)

- [x] **Tolerance is a policy input, not a compromise.**
      `retrieval::search` takes `SourceTolerance::{HardFail, Degrade}`:
      MCP keeps its typed-error contract (with the 027
      transient/permanent embed taxonomy preserved through the neutral
      `RetrievalError::Embed(StoreError)` variant); REST keeps
      `degraded: true` 200s. Neither surface's callers see a contract
      change.
- [x] **Shared-budget quirk dies.** Facts and events each get their own
      `top_k` cap in the shared core (MCP semantics). REST behavior
      change, intentional: an event-heavy query can no longer starve
      facts out of the page.
- [x] **REST `SearchHit.score` = the fused RRF value** — same number MCP
      reports, comparable within a response, still inside the
      contract-pinned 0..=1 range (RRF ≤ 1/61). Additive fields:
      `raw_score` (pre-fusion cosine/ts_rank) and `source_rank`;
      payloads additively gain `author` and `created_at`. All consumers
      verified serde-tolerant (klams-client, klams-mind reads, viewport
      TS mirror).
- [x] **REST gains the pipeline's guardrails and instrumentation**:
      MAX_QUERY_LEN=1024 (400 instead of silent embed), miss + sample
      logs, caller attribution from the bearer token's bound
      `agent_name` (`Option<Extension<AuthenticatedAuthor>>`).
      `top_k` stays clamp-not-reject, but the ceiling is now the
      pipeline's 50 (was 100).
- [x] **Filters go typed.** `RetrievalFilters` apply on typed fields in
      the core (knowledge: machine/repo/file/tags/source/created_at;
      facts: fact_type/source + payload keys for host/repo/file/tag —
      EnvFact payloads carry host). Two behavior *improvements* over
      the payload-key matching, recorded: `since`/`until` now filters
      knowledge by `created_at` (previously dropped ALL knowledge —
      the payload had no timestamp key), and `type`/`source` match the
      real typed columns. FTS over-fetches ×3 only when filters are
      active, keeping filtered pages full without taxing the common
      case.
- [x] **`memory_related`**: shared core adds duplicate collapse (+
      `copies` annotation), excludes copies of the seed's own content
      (same-content ≠ related), keeps the bare-ANN live-only guarantee
      (superseded = soft-deleted + pointer; the ANN filter already
      excludes them — measured in survey, no extra filtering needed).
      A superseded seed still resolves, by design.
- [x] **Bench question dissolves via the weld**: `tools/bench` speaks
      REST, and REST now IS the real pipeline (rerank included, ApiState
      carries the shared `TeiReranker`). No MCP-speaking bench needed;
      per-surface `op="rerank"` histograms cover the rest.
- [x] **`/memory/context` stays on `StoreHybridAdapter`** — the
      RankedRow payload is effectively ContextBuilder's input schema;
      pulling context onto the core's candidate stage is a separate
      sprint if ever justified. The adapter's Vector arm keeps its own
      boost gate / author-blind weight / collapse for that one path
      (residual duplication, accepted and documented in
      architecture.md §2.6).
- [x] **Sequential source fan-out** (MCP's shape) replaces REST's
      `tokio::join!` of vector+FTS — the embed+ANN leg dominates
      latency and the join bought little; one code path beats two.
- [x] **healthz reranker = visible, never fatal.** `Option<SubsystemStatus>`
      field, omitted when unconfigured, excluded from the aggregate;
      probe cache keyed by URL (unlike the other probes) so multiple
      in-process test routers can't cross-contaminate.
- [x] **Test-stack reranker = `BAAI/bge-reranker-base`** (CPU), port
      57071 — deliberately smaller than production's bge-reranker-v2-m3,
      same precedent as bge-small vs Qwen3 for the embedder (#732-style
      decision, documented in the compose file).

## Acceptance

- REST `/memory/search` and MCP `memory_search` produce results from the
  same core pipeline (curated stratum, provenance weights, boost gate,
  dedupe, rerank, configured fusion) — duplication inventory from the retro
  (2× provenance weights, 2× boost gate, 2× dedupe, 3× fuse, 3× kind-filter
  enums, …) is gone.
- `memory_related` goes through the shared core with dedupe + superseded
  semantics.
- `/healthz` reports reranker state without affecting overall status;
  klams-monitor/viewport coordination noted or done.
- Live-rerank test runs under `just test-integration`.
- Gate green; `just eval` before/after — baseline 21/21, 0 regressions;
  MCP ranking unchanged (this sprint moves code, it does not retune).

## Chronicle

### Survey (start of sprint)

- Dependency graph is already weld-shaped: `klams-core` depends on
  `klams-types` + `klams-store`; both `klams-api` and `klams-mcp` depend on
  `klams-core`. Moving `projection.rs` and the pipeline down creates no
  cycle. `klams-mcp` keeps `pub use klams_core::projection;` so its six
  `crate::projection::` call sites don't churn.
- The real pipeline is `memory_search.rs::run` (~490 lines of logic +
  helpers): validate → ANN (+×2 over-fetch) → curated stratum +
  boost-threshold gate → author resolution → FTS facts/events (separate
  caps) → tags filter → duplicate collapse → source-rank renumber →
  raw-score snapshot → rerank stage → weighted RRF with curated 4th list →
  truncate → miss log + sample log + caller metric.
- REST search (`handlers/search.rs`) is ~170 lines over
  `StoreHybridAdapter`: ×3 over-fetch, author-blind two-tier weight,
  collapse, `default_rrf()` hardcoded (search.rs:116), score clamped,
  `degraded` tolerance, filters via `RetrievalFilters`. `memory_related`
  is bare ANN + projection, no collapse, no superseded handling.
- **Filters constraint discovered**: REST search supports the full
  `RetrievalFilters` set (host/type/tag/repo/file/source/since/until —
  fixed in 033 after being silently ignored since 005). The shared core
  must carry filters or the weld would regress 033's fix. MCP passes
  defaults + its `tags` param.
- `/memory/context` stays on `StoreHybridAdapter` (its ContextBuilder
  bundling is a different shape; consuming the core's candidate stage is a
  future sprint if ever). Residual duplication on the adapter's Vector arm
  (boost gate, author-blind weight, collapse) is accepted and recorded —
  the *search* duplication dies, the context path is unchanged.
- #731 bench question resolves via #730 itself: once REST search runs the
  shared core pipeline (ApiState gains the reranker), `tools/bench`'s REST
  measurements measure the real pipeline. No MCP-speaking bench needed;
  histograms carry `op="rerank"` per surface.

### Implementation

- `klams-core/src/retrieval.rs` (new): the lifted pipeline —
  `search(store, SearchParams, &RetrievalConfig, SourceTolerance, caller,
  transport)` and `related(store, id, top_k)`, plus `RetrievalError` and
  all the stage helpers and their unit tests (36 tests, moved + new
  filter/tolerance/validation coverage). `projection.rs` moved from
  klams-mcp (re-exported there for path compatibility).
- `memory_search.rs`: 1429 → ~190 lines (args + `map_error` + envelope
  tests). `memory_related.rs`: shell over `retrieval::related`.
- REST: `handlers/search.rs` rewritten over the core; `scored_to_hit`
  rebuilds the adapter-era payload keys for wire compatibility.
  `ApiState` gains `fusion`/`reranker`/`rerank_window` (12 construction
  sites as the WI measured: main.rs + 11 test literals + the manual
  `Clone`). `main.rs` builds ONE `TeiReranker` shared by both surfaces.
- `/healthz`: `HealthSnapshot.reranker` (omitted-when-unconfigured),
  probe via the previously-dead `TeiReranker::health()`; contract tests
  pin visible-but-never-fatal. Viewport TS mirror + health table row
  added; klams-monitor verified tolerant (shared type, no code change).
- Test stack: `reranker` service in tests/docker-compose.test.yml,
  `TEST_RERANKER_URL` wired into `just test-integration`; mcp_rerank.rs
  gains a REST-side set-preservation test.
- Docs: architecture.md §2.5 (one pipeline, per-surface contract table,
  pre-036 history) + §2.6 (context = last adapter consumer), usage.md
  (healthz reranker block), setup.md (test-stack reranker).

### Test-run notes

- First `just test-integration` run tripped ONE failure:
  `us3_decay::tick_is_monotonically_non_increasing` — Postgres
  `deadlock detected` in `apply_decay_batch`, i.e. two decay ticks from
  parallel tests colliding. Nothing in this sprint touches decay or the
  write path; the test passes in isolation. Pre-existing parallelism
  flake, noted for a future breather (candidate: order fact ids in the
  batch UPDATE, or advisory-lock the decay tick in tests).
- The recipe's `*ARGS` land after `--` (libtest side), so
  `just test-integration --no-fail-fast` cannot work — the cargo-side
  rerun was done by hand. Not worth a recipe knob yet.
- Eval before-baseline against live 0.1.35: green, 0 regressions
  (the `known_open` "klams gotcha" query as expected).
