# Sprint 022 — scanner v2: chunks worth retrieving

**Status:** Active (started 2026-07-11 from korg proposal [korg:338];
covers klams WIs #320–#326, plus a CI fix from WI #327's comment)
**Version:** workspace PATCH → `0.1.22`
**Derives from:** [../planning/roadmap.md](../planning/roadmap.md) queue
entry 022 · [../planning/2026-07-crossroads.md](../planning/2026-07-crossroads.md)
§2.2 #1 + §5 (#3, #6, #7, #12)

## Goal

The chunk-quality sprint — the fix for the junk hits the crossroads
review observed live (`"## MCP tools"`, `"# PHASE 8 — Restore Data"`
returned to agents and answering nothing). 021 stopped the corpus
re-polluting itself; 022 makes the chunks themselves worth retrieving,
then re-indexes the ~94k corpus so the improvement is realized. This is
the prerequisite for 023's ranking work (perfect ranking over junk is
still junk) and the first half of the graph on-ramp (symbol extraction).

## Scope

### CI fix (WI #327 comment) — land first, independent

Main CI has been red since ≥018 while every PR is green. The `service`
job runs a **main-only** `cargo test --workspace -- --ignored` step; PRs
skip it. That step panicked on
`embeddings::tests::openai_embed_returns_configured_dim_vector`
([embeddings.rs:268](../../crates/klams-store/src/embeddings.rs)), which
`.expect()`ed `TEST_OPENAI_EMBED_URL` — a var CI never set. Fix: set
`TEST_OPENAI_EMBED_URL=http://127.0.0.1:57070/v1` + model in the CI
`--ignored` env (the test TEI already serves `/v1/embeddings`, verified
200 — gives the sprint-014 OpenAI-compat path real coverage), and harden
both env-gated integration tests to self-skip when their var is unset so
a future env change can't re-break main. *(Done + verified.)*

### #320 — Heading-only chunks (P0) · §5 #3

Section split on every heading with no minimum chunk size
([chunk.rs:74-84,118-130](../../crates/klams-scanner/src/chunk.rs)), and
`is_heading` matches `# ` code comments so Python/shell/TOML fragment on
every top-level comment ([chunk.rs:138-153](../../crates/klams-scanner/src/chunk.rs)).
Fix: markdown-only heading detection, a **minimum chunk size** (a bare
heading merges forward into its body), and heading-*path* context so a
chunk carries its section breadcrumb (`H1 > H2 > body`) not a bare
heading. Acceptance: the crossroads junk-hit examples return substantive
chunks.

### #321 — Newline-preserving normalization · §5 #6a

Scanner `normalize` trims every line (kills indentation); the API's
`normalize_text` collapses newlines to spaces. Stored chunks are one
long line. Make normalization newline/indentation-preserving end-to-end
so code and structured prose survive ingestion. Touches both
`crates/klams-scanner/src/chunk.rs::normalize` and
`crates/klams-api/src/handlers/knowledge.rs::normalize_text` — they must
agree (the content-hash dedupe depends on identical normalization).

### #322 — Chunk metadata on the wire · §5 #6b

Chunk `index` is computed but never transmitted; no language or
heading-path metadata reaches the store. Add index + language +
heading-breadcrumb fields to `IndexKnowledgeRequest`/the Qdrant payload
(schema addition) so neighbor expansion and "prepend section heading"
retrieval become possible, and so symbol/language is available to the
graph layer later.

### #323 — Code-aware chunking via tree-sitter · §2.2 #1

Chunk code by structure (functions/items) rather than blindly by line,
via tree-sitter (concept proven in-house in krag). Store symbol
names/language as payload. Language set scoped to what the corpus is
mostly made of (Rust, Python, markdown, and a couple more); everything
else falls back to the improved text chunker (#320/#321). Keep the
dependency surface tight.

### #324 — Cross-file dedupe hazard (P1) · §5 #7

The content-hash probe is global
([knowledge.rs:60-77](../../crates/klams-api/src/handlers/knowledge.rs)),
so an identical chunk in two files becomes one point owned by the first;
deleting that file removes it for both — and this interacts with 021's
delete-before-reindex. Scope dedupe per `source_file` (or track
multi-file ownership) so hygiene deletes can't silently drop a chunk
still live in another file.

### #326 — Golden real-file chunker tests · §5 #12

Chunker tests are synthetic-only today. Add golden tests over real
Rust/Python/shell/TOML/markdown fixtures asserting: minimum chunk size,
heading-path context, preserved newlines/indentation, code-aware
boundaries, and the specific crossroads junk-hit inputs producing
substantive chunks. Locks scanner v2 so it can't regress.

### #325 — TEI batch-embedding path + full re-index · §5 #12

The TEI batch-embedding path is unused and needed before a 94k re-index.
Wire it, then run the full re-index (clear cursor + rescan, which also
executes 021's delete-before-reindex per file). The 014 re-embed runbook
applies. This is the deploy-time step that realizes all the above on the
live corpus. Acceptance: crossroads junk-hit queries return substantive
chunks live.

## Acceptance

1. `is_heading` no longer matches code comments; a bare heading never
   becomes its own chunk; chunks carry a heading path. Golden real-file
   tests prove it (#326).
2. Stored knowledge preserves newlines/indentation (scanner + API
   normalization agree; dedupe hash stable).
3. Chunk index + language + heading-path travel to the store.
4. Code files chunk by structure with symbol payload.
5. Per-file dedupe: deleting one file cannot drop a chunk live in
   another.
6. Full corpus re-indexed; crossroads junk-hit examples return
   substantive chunks on the live MCP surface.
7. `just gate` green (fmt, clippy -D warnings, tests); **main CI green**.

## Sequencing

CI fix → #321 (normalization foundation) → #320 (heading/min-size, the
observable P0) → #323 (tree-sitter) → #322 (metadata on wire) → #324
(dedupe) → #326 (golden tests) → #325 (batch + re-index, last / deploy).

## Outcome (2026-07-11 — implemented, gate green)

All seven WIs + the CI fix landed on `022-scanner-v2` in seven commits;
`just gate` green, docker-gated integration tests verified live.

- **CI fix (#327 comment)** — main CI (red since ≥018) fixed: the
  main-only `--ignored` step panicked on an unset `TEST_OPENAI_EMBED_URL`.
  Set it (+ model) in CI so the openai-compat path gets real coverage,
  and hardened both env-gated tests to self-skip. Verified both ways.
- **#321** — shared `klams_types::normalize_chunk_text` (newline/indent
  preserving, idempotent); scanner + API agree.
- **#320** — language-aware chunker: markdown heading-path breadcrumbs +
  no bare-heading chunks; code/config split on blank lines (no
  code-comment fragmentation); min-size same-path merge.
- **#322** — chunk index/language/heading_path/symbols travel to the
  Qdrant payload (additive).
- **#323** — tree-sitter code chunking for Rust/Python with symbol
  extraction; falls back to the plain splitter.
- **#324** — content-hash dedupe scoped per source file; docker-gated
  `chunk_dedupe_scoping` test proves identical-chunk-in-two-files stays
  two points.
- **#326** — golden real-file chunker tests pinning the crossroads
  junk-hit scenarios.
- **#325** — `Embedder::embed_batch` (real TEI + openai-compat batch,
  tested hermetically + live). The scanner ingest stays per-chunk; the
  batch path is the primitive for a future bulk re-embed.

**Deploy-time (kubs0):** install 0.1.22 binaries; run the full re-index
(clear cursor + rescan — absorbs 021's one-time purge), documented in
[usage.md](../../docs/usage.md).

## Out of scope (deferred, tracked)

- Ranking unification (#328–#332) → sprint 023.
- Multi-vector embeddings / embedding-model upgrade → after 022 (roadmap
  Later); 022's batch path + the 014 runbook make it tractable.
- Graph memory spike (#323 is only the symbol-extraction half) → 025.
