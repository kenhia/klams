# klams Architecture

This document describes how klams is assembled at runtime: which
components exist, how they communicate, where their state lives, and how
they are deployed on the production host `kubs0`. It complements
[setup.md](setup.md) (provisioning) and [usage.md](usage.md)
(operator-facing recipes). It describes the **current** system; the
sprint-by-sprint history lives in `sprints/NNN-*/` and git history, with
inline attributions here ("sprint 029, #644") where the trail matters.

## 1. Components

```text
                       ┌─────────────────────────────┐
                       │  klams-viewport             │
                       │  (Windows / Linux / WSL)    │
                       │  Tauri 2 + SvelteKit        │
                       │  desktop UI                 │
                       └──────────────┬──────────────┘
                                      │ bearer over the tailnet;
                                      │ klams-client crate
                                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│ kubs0 (Linux x86_64)                                                │
│                                                                     │
│   ┌──────────────────────────────────────────────────────────────┐  │
│   │ klams-service (systemd, native binary, 127.0.0.1:7777)       │  │
│   │   ┌───────────────────────────────────────────────────────┐  │  │
│   │   │ klams-api     axum router, bearer auth + scopes,      │  │  │
│   │   │               validation, /healthz, /metrics          │  │  │
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ klams-mcp     MCP tool surface at /mcp (rmcp),        │  │  │
│   │   │               PublicMemory projection, scope gating   │  │  │
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ klams-core    bounded mpsc write queue + worker pool, │  │  │
│   │   │               hybrid retrieval + RRF fusion,          │  │  │
│   │   │               provenance weights, dedupe collapse     │  │  │
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ klams-store   Postgres (sqlx) | Qdrant (gRPC) |       │  │  │
│   │   │               TEI embedder | TEI reranker (HTTP)      │  │  │
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ Background    backup (pg_dump + qdrant snapshot),     │  │  │
│   │   │ tasks         fact decay, event summarization,        │  │  │
│   │   │               oversize-log prune, auth SIGHUP reload  │  │  │
│   │   └───────────────────────────────────────────────────────┘  │  │
│   └────┬───────────────┬───────────────┬───────────────┬────────┘  │
│        │               │               │               │           │
│        │ TCP 5432      │ gRPC 6334     │ HTTP 7070     │ HTTP 7071 │
│        ▼               ▼               ▼               ▼           │
│  ┌───────────┐   ┌───────────┐   ┌───────────┐   ┌────────────┐    │
│  │ postgres  │   │ qdrant    │   │ tei       │   │ reranker   │    │
│  │ (Compose) │   │ (Compose) │   │ (Compose) │   │ (Compose)  │    │
│  │ facts,    │   │ knowledge │   │ Qwen3     │   │ bge-       │    │
│  │ events,   │   │ vectors   │   │ embedder, │   │ reranker-  │    │
│  │ authors,  │   │ (v2, 1024 │   │ GPU       │   │ v2-m3, GPU │    │
│  │ logs      │   │  dims)    │   │           │   │            │    │
│  └─────┬─────┘   └─────┬─────┘   └─────┬─────┘   └─────┬──────┘    │
│        │               │               │               │           │
│        └── all four on user-defined bridge `klams-net` ────────────┘
│        │               │               │               │           │
│        ▼               ▼               ▼               ▼           │
│  ${KLAMS_DATA_ROOT}/postgres  /qdrant  /tei  (reranker shares /tei)│
└─────────────────────────────────────────────────────────────────────┘
```

Two more binaries feed the service from outside the process:
`klams-scanner` (systemd timer, hourly) and `klams-monitor` (systemd
service) — see §2.4.

### 1.1 Service crates (`crates/`)

| Crate | Role |
|-------|------|
| `klams-types` | Shared serde DTOs (`Fact`, `Event`, `KnowledgeItem`, `MemoryWrite`, `PublicMemory`, `ScoredMemory`, `HealthSnapshot`), plus shared policy types: `EmbedLimit` token estimation (`src/embed_limit.rs`), `DecayConfig` validation, auth config shapes. No I/O. |
| `klams-core` | The async heart: bounded mpsc queue + worker pool, `MemoryWrite` dispatch, hybrid retrieval + RRF fusion (`src/hybrid.rs`), provenance weighting (`src/provenance.rs`), query-time duplicate collapse (`src/dedupe.rs`), context bundling, summarization, decay worker, metrics registry. |
| `klams-store` | Storage adapters: `PostgresStore` (sqlx, compile-time checked), `QdrantStore` (gRPC), `TeiEmbedder` behind the `Embedder` trait (`src/embeddings.rs`; an `OpenAiCompatEmbedder` alternative is selected via `[embeddings] api`, sprint 014), `TeiReranker` (`src/rerank.rs`, sprint 030 #685). `CompositeStore` implements the one `Store` trait everything upstream consumes. |
| `klams-api` | `axum` router, bearer auth + per-route scope middleware, request validation, error → JSON mapping, REST handlers, `/healthz`, `/metrics`. |
| `klams-mcp` | The MCP tool surface (rmcp `StreamableHttpService` mounted at `/mcp`), projection to `PublicMemory`, scope gating, tool metrics. Generic over `Store` (`McpState<S: Store>`) since sprint 031 (#645) so MCP and REST share one write layer — enforced by `crates/klams-mcp/tests/no_concrete_store_reachthrough.rs`. |
| `klams-service` | The binary. Loads `klams.toml`, wires queue + workers + HTTP server + background tasks, owns the tokio runtime. |
| `klams-client` | Typed HTTP client. Used by the viewport's Tauri backend so the desktop app and any future Rust caller share one API contract. |
| `klams-scanner` | Non-agentic filesystem writer: walks configured roots, chunks, publishes to `/memory/knowledge/index` (sprint 003; §2.4). |
| `klams-monitor` | Non-agentic systemd-state writer: posts `Service` events on unit-state edges; optional kpidash `/healthz` reporter (sprint 003/010; §2.4). |

### 1.2 Viewport (`viewport/`)

Independent Cargo workspace + SvelteKit project. Tauri 2 native shell
hosts a static SvelteKit bundle; the Rust side exposes a small
`#[tauri::command]` surface that delegates to `klams-client`. Built on
Linux via `cargo-xwin` targeting `x86_64-pc-windows-msvc` (`just
viewport-build`); ships as a single `klams-viewport.exe` with no
installer. A native Linux build is also supported (`just
viewport-build-linux`) and runs unchanged under WSL Ubuntu via WSLg
— useful for headless verification before cutting a Windows release.

Runtime config (`bearer`, `service_url`) lives in
`%APPDATA%\klams\config\viewport.toml` on Windows and
`$XDG_CONFIG_HOME/klams/viewport.toml` on Linux; the bearer is
stored in the platform-native credential store via the `keyring`
crate (`windows-native` on Windows, `linux-native` / Secret Service
on Linux). The `--debug` CLI flag opens WebView devtools and enables
per-poll diagnostic logging to `%TEMP%\klams-viewport.log` (or
`/tmp/klams-viewport.log` on Linux); otherwise the app runs quietly.

The `custom-protocol` Tauri feature is enabled by default in
`viewport/src-tauri/Cargo.toml` so that bypass-CLI builds
(`cargo xwin build --release` via `just viewport-build`) still embed
the asset-protocol handler; without it the webview can't reach the
bundled SvelteKit assets and stays at `about:blank`.

Key routes: `/` dashboard (polls `/healthz`), `/activity` (sprint 008),
`/authors/{id}` drilldown, per-kind detail pages `/facts/{id}`,
`/events/{id}`, `/knowledge/{id}` (sprint 015), `/dissents` with diff +
promote/discard (sprint 002), `/preview` context-bundle preview
(sprint 005).

### 1.3 Stateful dependencies (Docker Compose)

[`deploy/docker-compose.yml`](../deploy/docker-compose.yml) defines four
always-on services plus two behind the `observability` profile, all
attached to a single user-defined bridge network `klams-net` with
deterministic DNS aliases. The default `bridge` network is intentionally
not used — rationale in
[research.md §13](../sprints/001-initial-mvp/research.md#13-docker-network).
Image tags and model ids are pinned in `compose.env` (template:
[`deploy/compose.env.example`](../deploy/compose.env.example)).

| Service | Container | Bind | Volume | Notes |
|---------|-----------|------|--------|-------|
| `postgres` | `klams-postgres` | `127.0.0.1:5432` | `${KLAMS_DATA_ROOT}/postgres` | Postgres 16. Container runs as uid 999 — never `chown -R` the data tree. Holds `facts`, `events`, `authors`, `dissents`, `summaries`, and the `search_miss` / `search_sample` / `oversize_write` logs. |
| `qdrant` | `klams-qdrant` | `127.0.0.1:6333/6334` | `${KLAMS_DATA_ROOT}/qdrant` | Pinned via `QDRANT_IMAGE_TAG`. Client/server minor versions must match. Holds the knowledge collection `knowledge_items_v2` (1024-dim, ~180k points; the pre-028 `knowledge_items` collection was dropped in sprint 032, #684). |
| `tei` | `klams-tei` | `127.0.0.1:7070` | `${KLAMS_DATA_ROOT}/tei` | Embedder: `Qwen/Qwen3-Embedding-0.6B` (`TEI_MODEL_ID`), CUDA image on the GPU via CDI ([`deploy/docker-compose.gpu.yml`](../deploy/docker-compose.gpu.yml)). Runs `--auto-truncate false` — a silently truncated **stored** chunk looks complete but is unfindable by its tail (standing decision, sprints 027/028) — and `--max-batch-tokens 32768` to match the model's context. |
| `reranker` | `klams-reranker` | `127.0.0.1:7071` | shares `${KLAMS_DATA_ROOT}/tei` | Second TEI container serving `BAAI/bge-reranker-v2-m3` over `POST /rerank` (sprint 030, #685). Same GPU via CDI. Runs `--auto-truncate` **on** — the opposite of the embedder, deliberately: this container only *scores* (query, text) pairs, nothing is stored, so truncation degrades one ranking signal gracefully. `--max-client-batch-size 64` must stay ≥ `[retrieval] rerank_window` (default 50). |
| `prometheus` | `klams-prometheus` | `observability` profile | — | Scrapes `klams-service:7777/metrics` ([`deploy/prometheus/prometheus.yml`](../deploy/prometheus/prometheus.yml)). |
| `grafana` | `klams-grafana` | `observability` profile | — | Dashboard from [`deploy/grafana/klams.json`](../deploy/grafana/klams.json); see §2.9. |

GPU note (sprint 028, #655): kubs0's RTX 4080 SUPER is exposed via CDI
(`/etc/cdi/nvidia.yaml`, generated with `nvidia-ctk cdi generate` —
regenerate after NVIDIA driver upgrades), not the legacy
`deploy.resources` runtime form.

## 2. Data flow

### 2.1 Write path — facts and events

Two entry surfaces, one write layer. REST (`POST /memory/facts`,
`POST /memory/events`) is the controller/operator surface; MCP
(`memory_add` with a fact payload, `memory_append_event`) is the agent
surface. Since sprint 031 (#645) both route through the same core
validation and store calls — `McpState` is generic over `Store`, and a
guard test forbids the MCP crate from reaching around the trait.

```text
caller ──POST /memory/facts──▶ klams-api ─┐        both surfaces run the
agent  ──MCP memory_add──────▶ klams-mcp ─┤        same validation registry
                                          ▼        (sprint 031, #645)
                          per-type validators + sanity rules
                                          │  malformed writes never touch
                                          ▼  a store (sprint 002)
                          klams-core enqueue (bounded mpsc)
                                          │
                                          ▼
                          worker pool ──▶ PostgresStore::upsert_fact_v2
                                          │  (trust-ranked; sqlx)
                            ┌─────────────┴─────────────┐
                            ▼                           ▼
                     persisted Fact               lower-trust write vs a
                     (version++)                  higher-trust canonical?
                                                  → diverted to `dissents`
                                                  (canonical unchanged;
                                                   dissent_count bumped)
```

* **Validation** — per-type validators plus universal sanity rules run
  before the write reaches the queue (sprint 002); MCP fact writes run
  the same registry (previously the v1 path had none — sprint 031).
  Rejections increment `klams_validation_rejections_total{rule}`.
* **Attribution** — the bearer token's `agent_name` resolves to an
  `authors` row and stamps `author_id` on every write (sprint 009;
  §3.2). Tokens without `agent_name` fall back to `system`.
* **Optimistic concurrency** — every fact carries a monotonically
  increasing `version`; writers supply `expected_version` and stale
  writes return HTTP 409 with the current version so retries can be
  mechanical (sprint 002).
* **Trust + dissents** — a lower-trust write contradicting a
  higher-trust canonical fact diverts to the `dissents` table instead of
  overwriting; the caller gets `path: "dissent"` and a `dissent_id`.
  Agents can also file *semantic* contradictions directly via the
  `dissent_propose` MCP tool (proposed correction + required `reason`,
  optional `contradicting_memory_id`; lands as `Source::AgentProposal`
  — sprint 015). Resolution is operator-only: viewport `/dissents` →
  `POST /memory/dissents/{id}/{promote|discard}`, gated at `Manage`
  scope (§3.2).
* **Durability** — facts and events are persisted to Postgres on the
  worker before the success counter increments.
* **Events are append-only** — no update, no soft delete (§2.3 applies
  to facts and knowledge only).
* **Write-policy introspection** — `GET /memory/policy` exposes the
  dedupe + decay + trust rules so callers need not read the TOML
  (sprint 003).

### 2.2 Write path — knowledge (agent writes)

Knowledge text is embedded and stored **only** in Qdrant; there is no
Postgres knowledge table. Entry surfaces: MCP `memory_add`
([`crates/klams-mcp/src/tools/memory_add.rs`](../crates/klams-mcp/src/tools/memory_add.rs))
and REST `POST /memory/knowledge/index`
([`crates/klams-api/src/handlers/knowledge.rs`](../crates/klams-api/src/handlers/knowledge.rs),
the scanner's route).

```text
agent ──memory_add──▶ ┌──────────────────────────────────────────────┐
scanner ──/index────▶ │ 1. token-size gate (Store::check_embed_size  │
                      │    → TEI POST /tokenize, exact count against │
                      │    [embeddings] max_input_tokens; reject     │
                      │    BEFORE the 202)                           │
                      │ 2. content-hash dedupe probe                 │
                      │    (find_knowledge_by_content_hash)          │
                      │ 3. similar-on-write probe (MCP only)         │
                      └───────────────────┬──────────────────────────┘
                                          ▼
                          klams-core enqueue (bounded mpsc)
                                          │
                                          ▼
                     worker: TeiEmbedder.embed(text) ── HTTP ──▶ tei
                                          │
                                          ▼
                     QdrantStore.upsert(vector, payload)
```

* **One token ceiling, three enforcement points** (sprint 027, #629).
  `klams_types::EmbedLimit` is the single definition of "will the
  embedder accept this text". The REST and MCP gates ask the model's
  own tokenizer — `Store::check_embed_size` → `Embedder::count_tokens`
  → TEI `POST /tokenize` (no forward pass, cheap) — and reject with
  `413` **before** enqueueing. That ordering is the point: the scanner
  advances its cursor on the `202` and the worker has no reply channel,
  so anything accepted here and rejected later would be lost silently.
  The scanner, which cannot reach TEI directly, uses the
  `EmbedLimit::estimate_tokens` character heuristic (documented as an
  approximation — real content spans 1.03 to >39 chars/token, so no
  conservative divisor exists); the embedder's own preflight uses
  `EmbedLimit::certainly_exceeds`, a provable WordPiece lower bound,
  so a rejection there is final. Production ceiling:
  `[embeddings] max_input_tokens = 32768` (Qwen3-Embedding-0.6B,
  verified live via TEI `/info`); the compiled-in default of 512 is
  the bge-small legacy for hermetic test stacks.
* **Failure classification** (sprint 027, #656). `StoreError` carries a
  `Transience`: embedder 4xx fails immediately with the response body
  captured, 5xx/connect/timeout retry, and Postgres failures classify
  by SQLSTATE. The MCP error mapper (`errors::from_store_error`)
  enforces the contract invariant: `retry_after_seconds` is present
  **iff** the error is transient. A worker-side embed failure
  increments `klams_writes_failed_total{type,reason}` — the silent-loss
  triangle (oversize accept → reply-less worker drop → advanced
  cursor) is closed at all three corners.
* **Oversize-write log** (sprint 027). Refused knowledge writes are
  recorded in `oversize_write` *including the full submitted text* —
  evidence for whether server-side chunking (#632) is ever worth
  building. Because it retains whole documents it is pruned on a daily
  timer (`prune_oversize_writes`, `crates/klams-service/src/main.rs`),
  unlike the operator-pruned search logs.
* **Content-hash dedupe** — an identical live chunk short-circuits the
  write (the probe excludes soft-deleted points, so a re-add of deleted
  content stores fresh — sprint 028, #642). Scanner ingest instead
  *attaches a copy* to the existing point (§2.4).
* **Similar-on-write** (sprint 029, #638). `memory_add` reuses its
  embedding for a curated-stratum probe and returns `similar_existing`
  (up to 5 hits at raw cosine ≥ 0.85: id, text head, author) so the
  writer can supersede instead of duplicating, at the only moment that
  check is cheap. Non-blocking, best-effort.
* **Volatility declaration** (sprint 029, #638). `memory_add` /
  `memory_update` / `memory_supersede` accept optional
  `volatility: "stable" | "volatile"`, stored in the point payload and
  consumed at query time (§2.5).

### 2.3 Knowledge lifecycle — supersede, update, delete

Agent-written knowledge gets the smallest sufficient verb set
(sprint 029, #638); facts amend via versioned upsert, events append,
scanner chunks re-scan.

* **`memory_supersede(id, text, tags?, volatility?)`** — the primary
  correction verb
  ([`crates/klams-mcp/src/tools/memory_supersede.rs`](../crates/klams-mcp/src/tools/memory_supersede.rs)):
  writes the replacement (carrying `supersedes`), then stamps the old
  point with the soft-delete pair plus `superseded_by`. Every retrieval
  filter hides the superseded point; `memory_admin_list_deleted` shows
  the pointer and `memory_admin_restore` undoes the hiding. A
  mid-flight failure rolls the replacement back (best-effort) and the
  error says exactly what state the store is in.
* **`memory_update(id, text?, tags?, volatility?)`** — in-place edit,
  id stable; text changes re-embed and re-hash. Authorship never
  changes.
* **Authorization** — both verbs sit at `Write` scope with the shared
  ownership gate (`authorize_curation`: own it, or hold `Manage`) that
  `memory_delete` uses; supersession *is* a delete plus a write, so it
  is deliberately one authorization decision. Both refuse
  non-agent-authored targets (`NOT_AGENT_AUTHORED`) — scanner chunks
  are corrected by re-scanning, not by hand.

**Soft-delete representation** (sprint 007): `deleted_at`
(timestamptz / Qdrant payload string) is NULL for live rows, set to the
UTC delete time for tombstoned ones; every read path applies
`deleted_at IS NULL` (or the Qdrant `is_empty("deleted_at")`
equivalent) unless an admin tool asks for the inverse.

| State | Postgres / Qdrant | `memory_search` | `memory_admin_list_deleted` |
|-------|-------------------|-----------------|------------------------------|
| live | `deleted_at` absent | yes | no |
| soft-deleted | `deleted_at = T`, `deleted_by_author_id = A` (+ `superseded_by` if superseded) | no | yes |
| hard-deleted | row/point removed | no | no |

Background contradiction detection and consolidation stay klams-mind's
job (WI-259 division of labor) — klams ships the primitives.

### 2.4 Non-agentic writers — scanner and monitor

```text
+----------------------+          POST /memory/knowledge/index
| klams-scanner.timer  |  ─────▶  +-----------------+ ──────────────▶ +---------------+
|  (OnUnitActiveSec=1h)|          |  klams-scanner  | POST /memory/   | klams-service |
+----------------------+          |   (--once run)  | knowledge/      |               |
                                  +-----------------+ delete ───────▶ |               |
                                                                      +---------------+
+----------------------+          POST /memory/events                        ▲
|  klams-monitor.svc   |  ─────▶  +----------------+ ────────────────────────┘
|  (Type=simple,       |          |  klams-monitor |  (Service events on
|   Restart=on-fail)   |          |  sd-poll loop  |   edge transitions)
+----------------------+          +----------------+
```

**Scanner.** `klams-scanner` walks the configured roots (production:
`~/src` on **both** `kubs0` and `kai`, which sync the tree; the
Obsidian vault was removed from the corpus in sprint 028, #657 —
rationale and revisit criteria in [setup.md](setup.md)). It prunes
heavy dependency/cache trees (`target`, `node_modules`, `.venv`,
`__pycache__`, …) before descent, honours `.gitignore` +
`.klamsignore`, and applies a file-type allowlist (sprint 021) so only
content worth retrieving — source, docs/prose, config prose — is
indexed.

* **Chunking is language-aware** (sprint 022;
  [`crates/klams-scanner/src/chunk.rs`](../crates/klams-scanner/src/chunk.rs)):
  markdown splits on headings with heading-*path* context; Rust/Python
  parse with tree-sitter and split at item boundaries carrying symbol
  names; everything else splits on blank lines. The markdown splitter
  is **fence-aware** (sprint 028, #639): `markdown_blocks` tracks
  fenced-code state per CommonMark rules, so a `# comment` inside a
  fence is body text, never a heading — pre-028 such comments emitted
  content-free chunks that scored up to 0.956 raw cosine on
  heading-echo queries. A markdown-only body floor
  (`MIN_BODY_CHARS = 40`, breadcrumb excluded) drops tiny sections
  whose breadcrumb outweighs their content.
* **Per-host cursor** — a local SQLite database
  ([`crates/klams-scanner/src/cursor.rs`](../crates/klams-scanner/src/cursor.rs)
  at `~/.local/state/klams/scanner.sqlite`) short-circuits unchanged
  files on `mtime`, then on content hash. A failed publish leaves the
  cursor unadvanced so the chunk is re-offered next scan.
* **Real repo names** (sprint 028, #640) — `repo` is derived per file
  (deepest ancestor with a `.git` entry, else the first path segment
  under the scan root), making the `repo` retrieval filter meaningful.
* **Content-only storage dedupe with copy bookkeeping** (sprint 028,
  #642). ONE Qdrant point per `content_hash`. The (host, file) identity
  is payload bookkeeping: `copies[]` ({machine, file, repo},
  authoritative) with derived keyword-indexed `machines[]` / `files[]`
  lists, and the singular `machine`/`file`/`repo` as the canonical copy
  (re-promoted when the canonical copy is deleted). A dedupe hit
  attaches the new location (`attach_copy`,
  [`crates/klams-store/src/qdrant.rs`](../crates/klams-store/src/qdrant.rs));
  `delete_knowledge_by_source_file` removes one copy and deletes the
  point only when the last copy goes. Bookkeeping is serialized by a
  process-wide mutex — Qdrant has no transactions, and a lost update
  here could delete a point a host still relies on. Pre-028 points
  synthesize their singular fields as their only copy.
* **Delete-before-reindex** (sprint 021) — a *changed* file triggers
  `/memory/knowledge/delete?source_file=…&machine=…` before its new
  chunks are published, so edits replace rather than accumulate stale
  points; a *vanished* file triggers the same delete. The `machine`
  parameter is required — it scopes the blast radius to the host that
  observed the change.

**Monitor.** `klams-monitor` polls `systemctl is-active` for a
TOML-configured unit list, diffs against the last poll, and POSTs only
**edge transitions** (`active↔inactive`, version changes) as
`Event(category=Service)` with the real hostname; steady-state polls
emit zero traffic. Known limitation (kwi #55): the monitor posts to
`klams-service` itself, so it cannot record that service's own *Down* —
the outage stays reconstructable from the gap to the recovery *Up*.
Behind the default-on `kpidash` cargo feature
([`crates/klams-monitor/src/kpidash.rs`](../crates/klams-monitor/src/kpidash.rs)),
an optional `[kpidash]` config section additionally polls `/healthz`
and publishes a `kpidash:services:<name>:<host>` Redis card — an
*external* observer on a separate Redis host, which side-steps the
kwi #55 self-dependency. The section is inert when omitted.

### 2.5 Read path — MCP `memory_search`

The agent-facing, eval-measured retrieval pipeline
([`crates/klams-mcp/src/tools/memory_search.rs`](../crates/klams-mcp/src/tools/memory_search.rs)).
Stages, in execution order:

1. **Validate** — non-empty query; `top_k` 1..=50 (default 10);
   optional `kinds` narrows which backends are queried.
2. **Embed the query** — `Store::embed_query` prepends
   `[embeddings] query_prefix` (the Qwen3 instruct prefix; asymmetric
   retrieval models prefix *queries*, never documents — sprint 028,
   #655) and calls TEI. An over-long query classifies as permanent
   `PAYLOAD_TOO_LARGE`, not an outage.
3. **Global ANN** — `search_knowledge` over-fetched ×2
   (`KNOWLEDGE_OVERFETCH`): ~44% of the corpus is duplicate cross-host
   content, so a plain top-k fetch routinely collapsed to half a page
   (sprint 026, #641).
4. **Curated-stratum ANN** (sprint 029, #644/#628) — a second, filtered
   search (`search_knowledge_curated`: `source = AgentProposal` AND no
   `machine`, live points only) with the same query vector.
   Agent-authored knowledge is ~100 points in a ~180k corpus, so a
   badly-phrased query can miss the curated target in ANY global top-k;
   the stratum's own rank list later enters fusion as a 4th source. The
   `machine` gate matters: scanned agent-session transcripts are
   `AgentProposal` *with* a machine and would otherwise flood the
   stratum.
5. **Query-relative boost gate** — stratum membership and the tier
   weight both require a raw cosine ≥
   `provenance::boost_threshold(top_raw)` =
   `max(0.45, 0.82 × top_raw)`
   ([`crates/klams-core/src/provenance.rs`](../crates/klams-core/src/provenance.rs);
   0.45 is the measured Qwen3 junk line, 0.82 the competitive
   fraction). Without it, topically-adjacent agent memories (raw 0.60)
   displaced genuine bulk answers (raw 0.75) — eligibility, not fusion
   arithmetic, is where relevance holds the line.
6. **Author resolution** — one batched lookup maps knowledge points to
   author records for projection and tier classification.
7. **Per-hit provenance weight** (sprint 029, #644) — each knowledge
   hit gets `ProvenanceTier::classify(source, agent_name, has_machine)`
   × `volatility_demotion(volatility, age_days)`. Three tiers:
   hand-authored (`memory_add` writes, w = 2.0) > machine-extracted
   (klams-mind session extracts, w = 1.5) > bulk scanner (w = 1.0).
   Volatile-declared memories keep full weight for a week, then decay
   with a 30-day half-life floored at 0.25 — demoted, never
   disappeared. Stable and undeclared memories never decay: scanner
   `created_at` is scan time, and silently burying stable truths is the
   worst failure mode. Weights scale RRF contribution; they never
   reorder hits within a source list.
8. **Facts + events FTS** — `search_text` (Postgres `ts_rank`), scored
   and ranked per source.
9. **Tag filter** — post-projection; a hit must carry *all* requested
   tags.
10. **Duplicate collapse** (sprint 026, #641) —
    `klams_core::dedupe::collapse_duplicates` groups knowledge hits by
    `content_hash` and keeps the best-ranked copy, **before** fusion so
    freed ranks compact. The survivor carries `copies` so nothing
    becomes unreachable. Facts/events carry no `content_hash` and are
    never collapsed. `source_rank`s are re-numbered contiguously over
    the list the caller receives.
11. **Raw-score snapshot** — per-source scores are captured by id
    before fusion overwrites them: `raw_score` on the output, and the
    miss-log signal, are about the cosine, not the fused value.
12. **Cross-encoder rerank** (sprint 030, #685) — if
    `[retrieval] reranker_url` is set, the knowledge candidates (global
    + curated, post collapse/tag-filter, up to
    `[retrieval] rerank_window` = 50) go to `POST /rerank`
    (bge-reranker-v2-m3, port 7071;
    [`crates/klams-store/src/rerank.rs`](../crates/klams-store/src/rerank.rs)).
    The stage reorders the knowledge within-source rank list plus the
    curated order — the *inputs* to weighted RRF — so provenance
    weights apply to the reranked order: the cross-encoder fixes
    semantic order within a tier, the weights still arbitrate across
    tiers. Facts/events are not submitted (JSON payloads, not prose).
    Best-effort by contract: one attempt, 5 s timeout, no retries; any
    failure serves the un-reranked order, logs a warning, and bumps
    `klams_rerank_skipped_total`. Config absent = stage off (the
    rollback switch). Measured live: ~34 ms median, ~43 ms p99; it took
    the eval from 19/21 to 21/21 by fixing curated-vs-curated
    inversions that per-tier weights cannot see.
13. **Weighted RRF fusion** — `klams_core::hybrid::fuse` (strategy from
    `[retrieval] fusion`, default RRF `k=60`) over four rank lists:
    knowledge, facts, events, curated stratum. Per-hit contribution is
    `w/(k+rank+1)`. RRF is scale-free — it consumes ranks, not scores —
    which is why it replaced the raw-score sort (History: pre-024 the
    merged sort mixed Qdrant cosine with unbounded `ts_rank` and
    structurally favoured knowledge; sprint 024 #329/#330 fixed the
    class). Ties break deterministically by source discriminant then id
    (sprint 029). Truncate to `top_k`.
14. **Instrumentation, fire-and-forget** — every search appends a
    `search_sample` row (query, caller, top **raw** score + its kind,
    hit count, kinds, duplicates collapsed — sprint 026, #643); a
    zero-hit or weak search (top raw < `LOW_SCORE_THRESHOLD` = 0.45,
    calibrated for Qwen3 in sprint 028) also appends a `search_miss`
    row and bumps the miss counter. `klams_mcp_search_total` is
    labelled with the calling agent.

Output: `Vec<ScoredMemory>` — `{ score, raw_score, source_rank,
memory }` envelopes over the `PublicMemory` projection (§3.1). Contract
note: `score` is an **RRF value, not a similarity** — not comparable
across queries, never threshold it; rank order is the meaningful
output. `raw_score` is the per-source score (cosine for knowledge,
`ts_rank` for facts/events).

### 2.6 Read path — REST `/memory/search` and `/memory/context`

The REST read paths are **not** the §2.5 pipeline. Both go through
`StoreHybridAdapter`
([`crates/klams-core/src/hybrid.rs`](../crates/klams-core/src/hybrid.rs)),
which shares some stages and lacks others. Unification is an open work
item; until then the divergence is:

| Stage | MCP `memory_search` | REST adapter paths |
|-------|--------------------|--------------------|
| Over-fetch | ×2 | ×3 |
| Query-relative boost gate | yes | yes (same `boost_threshold`) |
| Provenance weight | three tiers via author resolution | author-blind two-tier approximation (`adapter_knowledge_weight`: klams-mind extracts get the hand-authored weight) |
| Duplicate collapse | yes | yes (`collapse_knowledge_rows`, same key) |
| Curated stratum (4th source) | yes | **no** |
| Cross-encoder rerank | yes (config-gated) | **no** |
| Fusion strategy | `[retrieval] fusion` config | `/memory/search` **hardcodes** `FusionStrategy::default_rrf()` ([`crates/klams-api/src/handlers/search.rs`](../crates/klams-api/src/handlers/search.rs)); `/memory/context` honours the config via `ContextBuilder::with_fusion` |
| Filters | tag filter only (tool argument) | full `RetrievalFilters` (host / type / tag / repo / file / source / since / until) |

`POST /memory/search` fans out vector + FTS retrieves through the
adapter, fuses, and returns flattened `SearchHit`s (preview + payload —
a different shape from MCP's `ScoredMemory`). Degraded mode: if one
source fails the response still returns 200 with `degraded: true` and
the surviving hits. As of sprint 033 (#692) the request's `filters`
field is parsed into the same `RetrievalFilters` the context handler
uses and actually applied — it had been accepted and silently discarded
since sprint 005 (contract-tested now).

`POST /memory/context`
([`crates/klams-api/src/handlers/context.rs`](../crates/klams-api/src/handlers/context.rs))
is the token-budgeted bundler (sprint 005): `ContextBuilder`
([`crates/klams-core/src/context.rs`](../crates/klams-core/src/context.rs))
retrieves per section (facts / knowledge / events) through the same
adapter, fuses per-section with the configured strategy, token-counts
items (`cl100k_base` via `tiktoken-rs`, `chars_div4` fallback), and
greedy-fills each section under the caller's `token_budget`, marking
`truncated` when the budget was hit. Per-section degradation is
reported in-band (`SectionMeta`); only when every source is unavailable
does the endpoint return `503 + Retry-After`.

### 2.7 Other read surfaces

* **`event_search` (MCP)** — pure SQL over the events table; it never
  invokes the embedder (sprint 008 contract: cheap event lookup). Since
  sprint 033 it attributes the caller in the search counter and log,
  like `memory_search`.
* **`memory_related` (MCP)** — nearest-neighbour walk from a given
  memory.
* **`GET /v1/memories` (REST) + viewport `/activity`** — a uniform,
  globally newest-first `PublicMemory` stream over all three kinds via
  the single `Store::list_memories` method (sprint 008: "two surfaces,
  one query" — `event_search` paging and this route share the store
  code, so agent-visible and operator-visible numbers cannot diverge).
  Defaults to a 24-hour window; windows over 30 days return 400.
  Soft-deleted rows surface with `state=deleted` and their tombstone
  metadata.
* **`GET /v1/authors*`** — author list/detail plus
  `GET /v1/authors/{id}/memories` for the viewport drilldown.
* **Admin tools (MCP)** — `memory_admin_list_deleted`,
  `memory_admin_restore`, `memory_admin_hard_delete`, and the author
  registry verbs (§3.2), all `Admin` scope.

### 2.8 Background tasks

All in-process in `klams-service`; no external scheduler.

* **Fact decay** — a tokio interval
  (`[decay] task_interval_seconds`, default 3600) recomputes
  `decay_weight = 1 / (1 + λ · age)` per fact (hyperbolic, from total
  age — not compounded; #648), with per-`FactType` λ from
  `[decay.lambda]`, batched by `[decay] batch_size` (default 500).
  Idempotent — a missed tick just means the next covers a longer Δt.
  `DecayConfig::validate()` rejects bad config at startup (exit 2
  before binding the listener; sprint 005). Fact search ranks by
  `decay_weight × relevance`. Knowledge has **no** blanket decay — only
  the declared-volatility demotion of §2.5.
* **Event summarization** — `SummarizationTask` (sprint 005) reads a
  7-day event window, clusters by `(host, category, day_bucket)`, and
  upserts extractive headlines ("3x compile, 2x test") into the
  `summaries` table. Extractive only: the LLM client and
  `[summarization]` LLM keys were removed in sprint 032 (#647/#335) —
  no code path ever sent a completion request; `SummaryMechanism::Llm`
  survives in klams-types only so old rows deserialize.
* **Backups** (sprint 006) — once per UTC day
  (`[backup] window_start_utc`), `backup::run_once` flips
  `MaintenanceState.active`, fires the `started` status hook, takes a
  `pg_dump` then a Qdrant snapshot (both written as `.partial` and
  atomically renamed), prunes retention (newest `daily_count` dates +
  newest `weekly_count` Sundays, filename-as-truth), fires
  `finished`/`failed`, and clears the flag in a guard on both paths. A
  lockfile + startup recovery handle mid-run crashes. While a backup is
  in flight the `maintenance_check` middleware 503s non-critical writes
  with `Retry-After`; dissent promote/discard are the critical-write
  exceptions. The status hook is exec-with-JSON-on-stdin
  ([`crates/klams-service/src/backup/hook.rs`](../crates/klams-service/src/backup/hook.rs)),
  bounded by a timeout with SIGTERM grace; hook failure is
  observability, not control flow. Restore tooling and its non-empty
  guard (Postgres rows and Qdrant points probed separately —
  [`crates/klams-service/src/backup/restore.rs`](../crates/klams-service/src/backup/restore.rs))
  are operator recipes in [setup.md](setup.md). Note
  `ProtectSystem=strict` in the hardened units: any writable path
  outside `StateDirectory` needs an explicit `ReadWritePaths=` — the
  backup dir gained one in sprint 020 after the hardened unit silently
  broke nightly backups for 40 days.
* **Oversize-log prune** — daily timer, §2.2.
* **Auth reload** — SIGHUP re-reads `[[auth.tokens]]` and atomically
  swaps the grant table (WI #61); token rotation needs no restart.

### 2.9 Health and observability

* **`/healthz`** returns a `HealthSnapshot`: per-dependency probe
  results for **postgres, qdrant, and embeddings** (plus version — the
  patch segment is the sprint number, which is how the dashboard shows
  the deployed sprint at a glance — and maintenance state). The
  reranker is deliberately **not** health-checked today; since the
  rerank stage is best-effort a dead reranker degrades quality
  silently. That is a tracked gap (WI filed in sprint 033).
* **`/metrics`** (Prometheus). The authoritative series contract is
  [`deploy/grafana/SERIES.md`](../deploy/grafana/SERIES.md) —
  `crates/klams-service/tests/grafana_dashboard_json.rs` fails if the
  dashboard queries an undocumented series or the code declares one
  SERIES.md omits.
  Highlights: `klams_retrieval_duration_seconds{op, transport}`
  (search/context latency at every entry point, including
  `op="rerank"` for the cross-encoder stage), `klams_queue_depth`,
  `klams_writes_total` / `klams_writes_failed_total{type,reason}`,
  per-author MCP counters (`klams_mcp_writes_total`,
  `klams_mcp_deletes_total`, `klams_mcp_search_total` — search gained
  real caller labels in sprint 026, and `event_search` attribution in
  sprint 033), `klams_validation_rejections_total`,
  `klams_rerank_skipped_total`, backup runs/duration/size/last-success,
  summarization runs/lag.
* **Search quality logs** (Postgres): `search_miss` — zero-hit or
  weak-match searches, threshold 0.45 raw cosine (recalibrated per
  embedding model; History: it was 0.5 against bge-small — below that
  model's junk floor, so it never fired — then 0.80, then re-derived
  for Qwen3 in sprint 028) — and `search_sample`, every search's query
  + caller + top raw score (sprint 026, #643). These feed the eval
  suite (`just eval`, 21 queries, runner in klams-mind) that gates
  retrieval changes.
* **Grafana** — [`deploy/grafana/klams.json`](../deploy/grafana/klams.json),
  17 panels (queue, throughput, latency, errors, backup age,
  maintenance, summarization, per-author MCP activity, search misses,
  oversize/failed writes). Production install lives in ansible-k.
* **Logs** — structured `tracing`, JSON when `KLAMS_LOG_FORMAT=json`
  (the systemd unit sets this). The viewport polls `/healthz` on an
  exponential backoff capped at 60 s.

### 2.10 Document history

This document was restructured in sprint 033 (#692): the original
sprint-001 description plus fourteen delta sections (§2a–§2p, one per
sprint) were folded into the single current-state description above.
The delta-section trail lives in git history and in `sprints/NNN-*/`.

## 3. MCP surface and auth

### 3.1 Public projection

The only shape returned by MCP tools and the viewport author/activity
REST endpoints is `klams_types::PublicMemory` (sprint 007). The
internal `Fact`, `Event`, and `KnowledgeItem` types are **never**
serialized across the public boundary; the projection deliberately
omits `version`, `decay_weight`, `confidence`, `use_count`,
`last_used_at`, raw embedding vectors, and the internal `source` trust
tier. Knowledge projections carry `content_hash` (the collapse key),
`heading_path` / `language` / `chunk_index`, and `author.id` (ownership
reasoning without a round-trip) — sprint 026. The knowledge→public and
author mappings each live in one place
(`PublicMemoryContent::knowledge_from`, `PublicAuthorRef::from_record`);
they were previously hand-rolled at four call sites, which is how
payload fields got written but never projected.

The surface split is binding (sprint 015): the agent surface is
MCP-only; REST is the controller/operator surface (klams-mind uses REST
only for `GET /v1/memories` bulk reads and `/healthz`).

### 3.2 Scopes, tokens, attribution

Every MCP tool and every protected REST route is gated by a `Scope`
checked from the bearer token's grant. Four tiers — `Read`, `Write`,
`Admin`, `Manage` — and **scopes are flat, not hierarchical**:
`Scope::satisfies` is exact equality, so `Write` does not imply `Read`;
every grant lists what it needs. Full model: [auth.md](auth.md).

| Surface | Scope |
|---------|-------|
| `memory_search`, `memory_related`, `event_search`; REST reads (`/memory/search`, `/memory/context`, `GET /memory/*`, `/v1/authors*`, `/v1/memories`) | `Read` |
| `memory_add`, `memory_append_event`, `memory_delete`, `memory_supersede`, `memory_update`, `dissent_propose`, `register_author`; REST writes (`POST /memory/facts`, `/memory/events`, `/memory/knowledge/{index,delete}`) | `Write` |
| `memory_admin_*` (restore, hard_delete, list_deleted, list/remove/merge authors) | `Admin` |
| Cross-author curation: deleting/superseding/updating somebody else's memory; REST dissent promote/discard | `Manage` |

`Manage` gates *behaviour* rather than whole tools: self-management
needs only `Write` (`authorize_curation` — own it, or hold `Manage`).
Sprint 025 (#637) layered `require_scope` onto every protected REST
route (previously exactly one route checked, so any valid bearer could
bulk-delete knowledge) — see the route table in
[`crates/klams-api/src/router.rs`](../crates/klams-api/src/router.rs).

**Tokens.** `[[auth.tokens]]` issues per-purpose bearer tokens with a
scope list and an optional `agent_name` (strict charset, validated at
startup). The legacy single `bearer_token` is materialized as a grant
with all scopes, bound to `system`. Tokens hot-reload on SIGHUP
(§2.8). Multiple tokens may share an `agent_name` and resolve to the
same author.

**Attribution** (sprints 007/009/018). The `authors` table attributes
every memory to the agent that wrote it; `facts.author_id` /
`events.author_id` are NOT NULL FKs and Qdrant points carry
`author_id` in payload. The bearer's `agent_name` resolves through an
`AuthorBinding` cache to an `author_id` stamped on every write — on
the MCP surface the token-bound identity is authoritative for
`memory_add`, `memory_append_event`, and `dissent_propose`
(`BEARER_AUTHOR_TOOLS` in
[`crates/klams-mcp/src/tools/mod.rs`](../crates/klams-mcp/src/tools/mod.rs)),
so `register_author` is rarely needed. `register_author` dedupes on
`agent_name` (sprint 025, #636 — it previously minted a fresh UUIDv7
per call), and the `memory_admin_{list,remove,merge}_author*` verbs
cover inspection, block-if-owned removal, and transactional merge.

### 3.3 Transport

The MCP surface is mounted at `/mcp` on the same axum router as the
REST API (no separate listener):

```text
Router::new()
  .merge(protected_rest)      // /memory/*, /v1/* behind require_bearer
  .merge(public)              // /healthz, /metrics
  .nest("/mcp",
        klams_mcp::router(mcp_state, cfg.server.mcp_allowed_hosts)
            .layer(require_bearer))   // ← layer attached HERE
```

The `require_bearer` layer **must** wrap the `/mcp` sub-router
directly; layering on the outer router after `.nest(...)` does not
apply to nested services in axum 0.8. One shared `AuthState` backs both
gates, so a single token works for REST and MCP.

`klams_mcp::router` wraps rmcp's `StreamableHttpService` (JSON-RPC over
POST/GET, streamable HTTP/SSE) with a configurable Host-header
allowlist (`[server].mcp_allowed_hosts`); the default is empty —
`require_bearer` is the real access control, and bearer-less requests
get `401` before any tool sees them. No OAuth metadata is served; VS
Code-style `"type": "http"` clients pass a static
`headers.Authorization`.

## 4. Deployment topology on `kubs0`

```text
kubs0
├── /ai/klams/                              KLAMS_ROOT
│   ├── config/klams.toml                   service config (perm 0600)
│   ├── config/compose.env                  image/model pins + PG password
│   ├── data/                               KLAMS_DATA_ROOT
│   │   ├── postgres/                       uid 999:999
│   │   ├── qdrant/
│   │   └── tei/                            model cache (embedder + reranker)
│   └── (backups live at [backup].backup_dir — a separate filesystem)
│
├── systemd
│   ├── klams-service.service               (Type=simple, After=docker.service)
│   ├── klams-scanner.service               (Type=oneshot, `klams-scanner --once`)
│   ├── klams-scanner.timer                 (OnBootSec=5min, OnUnitActiveSec=1h)
│   └── klams-monitor.service               (Type=simple, Restart=on-failure)
│
└── docker (compose project: klams)
    └── network: klams-net (bridge)
        ├── klams-postgres
        ├── klams-qdrant
        ├── klams-tei                       (GPU via CDI)
        ├── klams-reranker                  (GPU via CDI)
        └── klams-prometheus / klams-grafana  (observability profile)
```

The split between **systemd-managed klams binaries** and
**Compose-managed dependencies** is deliberate:

* The service is a single Rust binary with no native deps beyond libssl
  — built on the host it runs on (`cargo build --release` +
  `install-systemd.sh`), easy to restart. It connects to its
  dependencies over the published loopback ports, so it needs no place
  on `klams-net`.
* Postgres, Qdrant and the two TEI containers have non-trivial
  image/version management that Compose handles via `compose.env` pins.
* All three units share a hardening profile (`NoNewPrivileges`,
  `ProtectSystem=strict`, `ProtectHome`); they declare
  `After=/Wants=docker.service` because the stateful dependencies live
  in Docker. `install-systemd.sh` is idempotent, supports `--dry-run`,
  and rotates the previous binary to `<bin>.prev` so `just rollback`
  works.
* `klams-service.service` raises `LimitNOFILE=65536`, and a per-peer
  `ConnectionLimits` tower layer caps in-flight requests per remote IP
  and trims idle keep-alives (sprint 009, kwi #26 — a loopback
  CLOSE_WAIT leak exhausted fds under sustained traffic; validated by
  an 18-hour soak, `tools/soak/`).

Rationale in
[research.md §3](../sprints/001-initial-mvp/research.md#3-klams-service-deployment).

### 4.1 Network exposure

* `klams-service` binds **`127.0.0.1:7777`** (`listen_addr` in
  [`deploy/config/klams.example.toml`](../deploy/config/klams.example.toml)).
  Off-host access is via **`tailscale serve`**, which terminates TLS
  and proxies to loopback — the reachable address is
  `https://kubs0.encke-wahoo.ts.net:7777`, on the tailnet only.
  (History, sprint 032 #648: earlier revisions claimed `0.0.0.0:7777` +
  a UFW subnet rule; `0.0.0.0` was abandoned because it conflicted with
  `tailscaled` already holding :7777, and the access boundary is the
  tailnet.)
* Compose dependencies are bound to `127.0.0.1` only; they are reached
  by the service over loopback and never exposed to the LAN.
* All inter-container traffic stays on the `klams-net` bridge.

### 4.2 Secrets

* Bearer tokens: in `klams.toml` (file mode `0600`), constant-time
  compared on every request.
* Postgres password: in `compose.env` (mode `0600`) and inlined into
  the service's `postgres.url`.
* TLS is terminated by `tailscale serve`; the service itself speaks
  plain HTTP on loopback.

## 5. Where to look next

* End-to-end provisioning steps: [setup.md](setup.md).
* Day-to-day operator recipes (start/stop, log inspection, backups,
  restore, viewport install): [usage.md](usage.md).
* Auth model in full: [auth.md](auth.md).
* Metrics series contract: [deploy/grafana/SERIES.md](../deploy/grafana/SERIES.md).
* Per-sprint rationale and contracts: `sprints/NNN-*/` (sprints 001–012
  keep the retired spec-kit layout; 013+ use `sprint.md`).
