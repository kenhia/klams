# Phase 0 Research: Advanced Retrieval and Summarization

This document records the decisions reached for Phase 4 (sprint 005). All
`NEEDS CLARIFICATION` markers in the spec were resolved by the user
before planning began; the entries below capture both those decisions and
the planning-level decisions made during this Phase 0 pass.

## D-001 — Token-cost heuristic (resolves FR-002)

- **Decision**: Use `tiktoken-rs` with the `cl100k_base` encoder as the
  canonical token counter. Fall back to `chars / 4` when the encoder
  fails to load (missing data files, unsupported architecture) or when
  `[tokens] mode = "fallback"` is set in config. Report the active
  encoder in the `/memory/context` response envelope and at `/healthz`.
- **Rationale**: cl100k_base matches the chat tokenizers used by
  OpenAI- and Anthropic-family models, which is what the Phase 6 MCP
  clients will see when consuming klams's bundles. The fallback keeps
  the binary functional in environments where the tiktoken data files
  are absent (smoke tests, restricted images).
- **Alternatives considered**:
  - Pure `chars / 4`: ships zero new deps but produces budget overruns
    of 30–40% for code-heavy content. Rejected as the default.
  - Configurable selector with no `tiktoken` default: punts the choice
    to operators. Rejected — operators won't tune what they can't see.
  - `tokenizers` crate with a downloaded BPE: heavier, model-family
    specific, more moving parts. Rejected.

## D-002 — Hybrid fusion strategy (resolves FR-005)

- **Decision**: Reciprocal Rank Fusion (RRF) is the default. The
  formula is `score(d) = Σ_s 1 / (k + rank_s(d))` summed across
  sources `s ∈ {vector, fts, metadata-prefilter}`. Default `k = 60`
  (the published RRF default). Weighted score blending is available
  as an opt-in alternative with per-source weights and z-score
  normalization, selected by `[retrieval] fusion = "weighted"` in
  config. Cross-encoder reranking is explicitly out of scope.
- **Rationale**: RRF needs no cross-source score normalization, has
  one tunable parameter, and is empirically competitive with weighted
  blends in IR literature. Shipping it as the default means the
  hybrid path works the moment the code lands; weighted blending
  exists as a tuning escape hatch for the rare cases where one
  source's ranking is demonstrably better than another's.
- **Alternatives considered**:
  - Weighted blending as default: requires score normalization across
    Qdrant cosine similarity, Postgres `ts_rank_cd`, and metadata
    pre-filters that have no score. Rejected for default; kept as
    opt-in.
  - Cross-encoder rerank: best quality, but introduces a model
    dependency and a latency tail incompatible with SC-003. Deferred
    to a future phase.

## D-003 — Summarization mechanism (resolves FR-010)

- **Decision**: Hybrid extractive-first / LLM-fallback. For events,
  the extractive path produces top-K category counts plus
  time-bracket headlines (e.g. `"qdrant up→down→up 7× over 14d"`).
  For knowledge clusters, the extractive path picks the longest
  representative chunk and concatenates titles. When the extractive
  output exceeds a documented quality threshold (output token cost
  exceeds 60% of the raw cluster's cost — i.e. the "summary" isn't
  smaller; OR coverage falls below 50% of the cluster's distinct
  vocabulary), the task falls back to a local LLM call against
  Phi-3-medium served by Ollama on `kubs0`'s GPU. The LLM fallback
  is optional at the config level (`[summarization] llm_fallback =
  true|false`) so the service still runs cleanly when Ollama is
  down. Each summary record carries a `mechanism` field
  (`"extractive"` or `"llm"`).
- **Rationale**: Events are structured and compress well via
  counting + bracketing; prose chunks need true summarization for
  good output. The fallback ordering means the hot path never
  depends on Ollama being up.
- **Alternatives considered**:
  - Pure extractive: ships fastest, but knowledge digests are weak.
    Rejected as the only option; kept as the first stage.
  - Pure LLM: best quality, but a hard Ollama dependency and a
    latency tail that fights SC-004. Rejected.

## D-004 — RRF `k` parameter

- **Decision**: `k = 60` default, exposed as `[retrieval] rrf_k`.
- **Rationale**: 60 is the value Cormack/Clarke/Buettcher 2009
  recommended and the value most retrieval libraries default to.
  Configurable so a future tuning pass can adjust without a code
  change.

## D-005 — Event-cluster definition for summarization

- **Decision**: An event cluster is `(host, category, day-bucket)`
  where `category` is the first dotted prefix of the event payload's
  category field (e.g. `service.up` and `service.down` cluster as
  `service.*`) and `day-bucket` is the UTC date of `created_at`.
  Clusters with `count >= 50` events in one day are eligible for
  summarization. The threshold is configurable.
- **Rationale**: Day-bucketing is what Ken will read; per-category
  bucketing keeps summaries coherent; 50 is a defensible "noise
  floor" for service flap chatter.

## D-006 — Stale-knowledge thresholds

- **Decision**: A knowledge chunk is stale when
  `last_used_at < now - 90d` AND `updated_at < now - 90d`. Stale
  chunks are clustered by `(repo, file-prefix(2))` (e.g.
  `klams/specs`) and digested when a cluster has ≥ 20 stale chunks.
  Threshold and time windows are configurable.
- **Rationale**: 90 days is long enough to skip transient lulls but
  short enough to actually compact aging content. The
  `(repo, file-prefix)` cluster definition keeps related notes
  together.

## D-007 — Decay-config validation behavior (FR-007)

- **Decision**: At service start, after loading
  `DecayConfig::lambda`, validate each entry: `λ` must be finite,
  non-negative, and the type key must be a recognized `FactType`.
  On any failure, log an actionable error naming the offending key
  and exit non-zero. Emit a single `INFO` log line listing the
  effective per-type table on successful validation. No SIGHUP
  reload in this sprint — restart-only, documented.
- **Rationale**: `DecayConfig` already exists from sprint 002 and
  reads from TOML; the gap is validation and observability. Shipping
  reload would expand surface area and risks for limited gain;
  restart on a homelab service is fine.

## D-008 — Summary storage

- **Decision**: `EventSummary` stored in a new Postgres table
  `summaries` (one row per cluster) with columns
  `id, kind ('event'), host, category, day_bucket, source_count,
  source_ids uuid[], summary_text, mechanism, generated_at,
  invalidated_at`. `KnowledgeDigest` stored in Qdrant alongside
  source chunks with payload `kind = "digest"` and metadata
  `source_ids`, `mechanism`. Both surface through the existing
  retrieval path; the context builder picks raw vs summary based on
  budget and matched-set size.
- **Rationale**: Events live in Postgres already; summaries belong
  there too for easy joins/back-references. Knowledge digests need
  vector search to be retrievable, so Qdrant is the right place; a
  flag on the payload is cheaper than a parallel collection.
- **Alternatives considered**:
  - Both in Postgres: would require a parallel embedding column or
    a sidecar Qdrant entry anyway. Rejected.
  - Both in Qdrant: events are not vector-searched. Rejected.

## D-009 — Viewport budget-slider transport

- **Decision**: Plain debounced HTTP POST per slider-stop. No SSE,
  no WebSocket. Debounce at 250 ms.
- **Rationale**: SC-006 wants "perceived-instant"; a 4 000-token
  bundle on local network is sub-100 ms in practice. SSE is overkill
  here and adds reconnect logic. Revisit if user testing demands it.

## D-010 — Ollama provisioning on `kubs0`

- **Decision**: Ollama is already provisioned on `kubs0` by
  `ansible-k`. Sprint 005 only needs to ensure Phi-3-medium is
  pulled on that host (verify via `ansible-k` role or one-shot
  `ollama pull phi3:medium`) and that klams-service can reach it.
  The klams-service binary does not install or manage Ollama; it
  speaks HTTP to it at the configured `[summarization] ollama_url`.
  If the model is missing or the endpoint is unreachable, the LLM
  fallback is off automatically (the client probes once on task
  start and caches the result for the cycle).
- **Rationale**: klams owns its data plane; ansible-k owns host
  provisioning. This split is consistent with sprint 003's
  systemd-via-Ansible decision.

## D-011 — `/memory/search` change scope

- **Decision**: `/memory/search` keeps its request and response
  shape. Internally it switches to call the same hybrid retriever
  the new `/memory/context` builder uses, so existing clients see
  improved ranking automatically (FR-012).
- **Rationale**: SDD prefers visible improvements over silent ones,
  but the response shape is a contract. Keeping the shape stable
  means the controller and viewport's existing search pane don't
  need a coordinated update.

## Open items deferred to backlog

- SIGHUP/file-watch reload of `[decay.lambda]` (currently restart-only).
- Per-section budget overrides on `/memory/context` (current API
  takes a single `token_budget`; per-section floors are computed
  by the builder, not specified by the client).
- Cross-encoder rerank stage.
- "Explain this ranking" affordance in the viewport.

These are noted in [specs/planning/backlog.md](../planning/backlog.md)
under the dedupe/decay viewport item or added as new entries during
implementation if they surface as friction.
