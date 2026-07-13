# Sprint 024 — one ranking: fusion unification + eval enablement

**Status:** Active (started 2026-07-13 from korg proposal [korg:339];
covers klams WIs #328–#332)
**Version:** workspace PATCH → `0.1.24`
**Derives from:** [../planning/roadmap.md](../planning/roadmap.md) queue
entry 024 · [../planning/2026-07-crossroads.md](../planning/2026-07-crossroads.md)
§5 (#2, #5, #9, #12) + §2.3 — the 016-deferred work, now due with agents
live.

## Goal

Make retrieval rank **one way**. Today three merge implementations
disagree, and the real-traffic surface (MCP `memory_search`) uses the
worst — a raw sort across incomparable score scales (Qdrant cosine vs
Postgres `ts_rank * decay * confidence * ln-use`), so knowledge
structurally outranks facts/events regardless of relevance. Converge on
`klams_core::hybrid::fuse` (RRF — already the `/memory/context` path),
route MCP through the `Store`/adapter seam so a third source can be added
in one place later, retire the config that lies about fusion, and lock it
all with hermetic tests that can't be skipped off-main.

## Scope

### #328 — One ranking (P0) · §5 #2

`memory_search` sorts raw scores across incomparable scales
([memory_search.rs:235-240](../../crates/klams-mcp/src/tools/memory_search.rs)).
Rebuild the merge on `hybrid::fuse(..., Rrf { k })`: each source's hits
are already ranked within-source (Qdrant cosine desc, Postgres `ts_rank`
desc — captured as `source_rank`); feed those per-source ranked lists to
RRF, order the `ScoredMemory` projections by the fused result, and set
`score` to the fused value. Same RRF the REST `/memory/context` path
uses — the REST `/memory/search` round-robin
([search.rs:104-156](../../crates/klams-api/src/handlers/search.rs)) is
reconciled onto the same fusion so all three merge paths agree.
Acceptance: a fact and a knowledge hit at equal within-source rank fuse
to comparable positions; knowledge no longer structurally wins.

### #329 — Route klams-mcp through the Store/adapter seam (P1) · §5 #5

MCP tools reach into `state.store.postgres` / `.qdrant` / `.embedder`
concretely ([tools/mod.rs:88-92](../../crates/klams-mcp/src/tools/mod.rs),
`memory_search.rs`) while klams-api is generic over `trait Store`. A
third retrieval source (025 lexical) would need wiring in ≥3 places.
Route the retrieval path through `StoreHybridAdapter`
([hybrid.rs](../../crates/klams-core/src/hybrid.rs)) / the `Store` trait
so fusion has one seam. Scope pragmatically: the fusion rewrite (#328)
is the lever; move `memory_search`'s source fetches onto the trait
without regressing the rich author projection.

### #330 — Wire or delete the dead `[retrieval] fusion` config (P1) · §5 #9

`[retrieval] fusion` is parsed + validated but never wired —
`ContextBuilder::with_fusion` has no production caller
([config.rs:257-293](../../crates/klams-service/src/config.rs),
`main.rs:169-172`). Wire it to the unified path, or delete it so config
can't lie. Prefer wiring (one knob, the fusion `k`), else delete.

### #331 — Hermetic merge-invariant tests + un-gate the 017 invariants · §5 #12

Fusion has no test with realistic cosine+`ts_rank` magnitudes, and the
017 rank invariants are DB-gated so a fusion regression passes branch
CI. Add hermetic merge-invariant tests (realistic cross-scale
magnitudes, no DB) so ranking can't regress off-main.

### #332 — klams-side surface for klams-mind's identifier-heavy eval · §2.3

Expose whatever klams-side surface klams-mind's eval harness needs to run
an identifier-heavy query suite (error codes, hostnames, function names,
config keys) against the live service — turning "is lexical search
valuable" into a number that feeds the §2.1 decision (sprint 025). Pairs
with klams-mind's harness (korg #269).

## Acceptance

1. MCP `memory_search`, REST `/memory/search`, and `/memory/context` all
   rank via `hybrid::fuse` (RRF); knowledge no longer structurally
   outranks facts/events at equal within-source rank.
2. Retrieval routes through the `Store`/adapter seam — a new source is a
   one-place addition.
3. `[retrieval] fusion` is wired (or gone); no dead config.
4. Hermetic, non-DB-gated merge-invariant tests cover realistic
   magnitudes; `just gate` green; main CI green.
5. klams-mind's identifier-heavy eval can run against the live service.

## Out of scope (deferred, tracked)

- Lexical knowledge source / the "Qdrant or OpenSearch" decision →
  sprint 025 (this sprint produces the eval surface + the ranking seam it
  needs).
- Multi-host (023, shipped), scanner v2 (022, shipped).
