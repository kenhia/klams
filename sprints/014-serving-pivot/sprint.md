# Sprint 014 — Serving pivot

**Branch:** `014-serving-pivot`
**Type:** feature — decouple klams from TEI/Ollama-native APIs so model
serving is swappable (vLLM-ready) by configuration.
**Seed:** [roadmap](../planning/roadmap.md) entry 014; decision record
[wi259-recommendation.md](../planning/wi259-recommendation.md).

## Goal

klams's only two model-serving touchpoints speak vendor-native
dialects: `TeiEmbedder` (TEI `POST /embed`) and `OllamaClient`
(Ollama `POST /api/generate`). After this sprint both speak the
OpenAI-compatible surface **as an option selected in `klams.toml`** —
no rebuild to change serving engines — and the re-embed procedure for
a future embedding-model change is written down and rehearsed.

## Scope

1. `Embedder` trait in `klams-store`; `CompositeStore.embedder`
   becomes `Arc<dyn Embedder>`. `TeiEmbedder` stays the default impl;
   new `OpenAiCompatEmbedder` (`POST {url}/embeddings`, OpenAI
   request/response shapes, optional bearer key).
2. `[embeddings] api = "tei" | "openai"` config selector (default
   `tei` — zero-change for the deployed service). `vector_dim` was
   already config-driven end-to-end (config → Qdrant collection →
   `expected_dim` check); verified, no code needed there.
3. Replace `OllamaClient` with `OpenAiChatClient`
   (`POST {url}/chat/completions`, probe via `GET {url}/models`).
   Config keys `[summarization] llm_url` / `llm_model` with serde
   aliases for the old `ollama_url` / `ollama_model` names.
4. Re-embed runbook (model/dim change procedure) in this sprint dir.
5. Docs: architecture/setup/usage + example config updated.

## Acceptance

- `just gate` green.
- New clients unit-tested against wiremock; live-gated tests pass
  against the kubs0 stack (`TEST_OPENAI_EMBED_URL`, `TEST_OPENAI_CHAT_URL`).
- Flipping `api = "openai"` with `url` pointed at TEI's own
  `/v1` route round-trips an embed on the live stack (verified via the
  gated test — same engine, new dialect, proving the config path).
- Re-embed runbook written; rehearsal path documented against the
  scale fixture.

## Decisions

- **Embedding topology (roadmap item 4):** embeddings stay **local to
  kubs0**. TEI remains the engine (it already exposes the
  OpenAI-compatible `/v1/embeddings` route — verified live 2026-07-06),
  so the `api = "openai"` selector covers both "TEI spoken via the
  standard dialect" today and "vLLM/kvllm" later purely by URL. The
  write path keeps zero cross-machine dependencies; kai/kvllm is for
  the heavier chat-model calls, which degrade gracefully
  (summarization falls back to extractive).
- **Embedding model:** stays `BAAI/bge-small-en-v1.5` @ 384-dim this
  sprint. A model upgrade is a separate, deliberate re-embed event —
  the runbook is the deliverable here, not the migration itself.
- **Chat URL convention:** `llm_url` / `[embeddings] url` (when
  `api = "openai"`) must be the OpenAI-compat base **including**
  `/v1` (e.g. `http://127.0.0.1:11434/v1`, vLLM
  `http://kai:8000/v1`). Deployed configs carrying the old
  `ollama_url` keep parsing (alias) but need the `/v1` suffix added
  at deploy time — called out in the deploy notes below.

## Ride-alongs

- kwi #33 (`bench-clean` Qdrant `?wait=true`): **already fixed** —
  `justfile` `bench-clean` has carried `?wait=true` since sprint 009's
  author-based rewrite. No code change; close the work item.

## Deploy notes (operator steps at ship time)

1. Update `/ai/klams/config/klams.toml` on kubs0:
   `[summarization]` → `llm_url = "http://127.0.0.1:11434/v1"`
   (was `ollama_url` without `/v1`; the old key still parses but the
   path must gain `/v1`).
2. `[embeddings]` needs no change (`api` defaults to `tei`).
3. `just install-systemd` + restart; confirm
   `klams_summarization_runs_total{mechanism="llm"}` still increments
   and `/healthz` embedder probe stays green.

## Chronicle

- (2026-07-06) Sprint opened. Live probes confirmed: TEI at
  `127.0.0.1:7070` answers `/v1/embeddings` (OpenAI shape, 384-dim);
  Ollama at `127.0.0.1:11434` answers `/v1/models` listing
  `phi3:medium`. kwi #33 found already fixed in the justfile.
- (2026-07-06) Found during survey: `vector_dim` was already
  config-driven end-to-end (config → `QdrantStore::connect` collection
  bootstrap → embedder `expected_dim` check) — roadmap item 2 needed
  only the runbook, no code.
- (2026-07-06) Implementation landed: `Embedder` trait +
  `OpenAiCompatEmbedder` in `klams-store/src/embeddings.rs`
  (wiremock-tested: parse, dim-mismatch, bearer key, 5xx retry,
  `/models` health); `CompositeStore.embedder` → `Arc<dyn Embedder>`
  (MCP tools' direct `store.embedder.embed(..)` calls unaffected);
  `[embeddings] api` selector wired in `main.rs`; `OpenAiChatClient`
  replaces `OllamaClient` (probe keeps the exact-or-`name:`-prefix
  model match; wiremock-tested); `[summarization]` keys renamed with
  aliases. Config tests cover the api selector + legacy-alias parse.
- (2026-07-06) Verification: `just gate` green. Live-gated tests green
  against the production stack — including
  `openai_embed_returns_configured_dim_vector` pointed at TEI's own
  `/v1` route (`TEST_OPENAI_EMBED_URL=http://127.0.0.1:7070/v1`),
  which is the acceptance case: same engine, new dialect, config-only
  swap. Re-embed runbook written
  ([re-embed-runbook.md](re-embed-runbook.md)); scale-fixture
  rehearsal documented there for a future model change.
