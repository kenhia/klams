# klams Architecture

This document describes how the klams MVP is assembled at runtime: which
components exist, how they communicate, where their state lives, and how
they are deployed on the production host `kubs0`. It complements
[setup.md](setup.md) (provisioning) and
[usage.md](usage.md) (operator-facing recipes), and is the operator-
oriented counterpart to the formal design records in
[specs/001-initial-mvp/plan.md](../specs/001-initial-mvp/plan.md) and
[specs/001-initial-mvp/research.md](../specs/001-initial-mvp/research.md).

## 1. Components

```text
                       ┌─────────────────────────────┐
                       │  klams-viewport (Windows)   │
                       │  Tauri 2 + SvelteKit        │
                       │  reads-only desktop UI      │
                       └──────────────┬──────────────┘
                                      │ HTTPS-style bearer over plain
                                      │ HTTP on LAN; klams-client crate
                                      ▼
┌─────────────────────────────────────────────────────────────────────┐
│ kubs0 (Linux x86_64)                                                │
│                                                                     │
│   ┌──────────────────────────────────────────────────────────────┐  │
│   │ klams-service (systemd, native binary)                       │  │
│   │   ┌───────────────────────────────────────────────────────┐  │  │
│   │   │ klams-api     axum router, bearer auth, validation,   │  │  │
│   │   │               error mapping, /healthz, /metrics       │  │  │
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ klams-core    bounded mpsc write queue + worker pool, │  │  │
│   │   │               MemoryWrite dispatch, dedupe hashing    │  │  │
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ klams-store   Postgres (sqlx) | Qdrant (gRPC) |       │  │  │
│   │   │               TEI HTTP embedding adapter              │  │  │
│   │   └───────────────────────────────────────────────────────┘  │  │
│   └───────┬──────────────────────┬──────────────────────┬────────┘  │
│           │                      │                      │           │
│           │ TCP 5432             │ gRPC 6334            │ HTTP 7070 │
│           ▼                      ▼                      ▼           │
│   ┌──────────────┐        ┌──────────────┐       ┌──────────────┐   │
│   │ postgres     │        │ qdrant       │       │ tei          │   │
│   │ (Compose)    │        │ (Compose)    │       │ (Compose)    │   │
│   │ facts,events │        │ knowledge    │       │ embeddings,  │   │
│   │              │        │ vectors      │       │ optional GPU │   │
│   └──────┬───────┘        └──────┬───────┘       └──────┬───────┘   │
│          │                       │                      │           │
│          └────── all three on user-defined bridge `klams-net` ──────┘
│          │                       │                      │           │
│          ▼                       ▼                      ▼           │
│   ${KLAMS_DATA_ROOT}/postgres   /qdrant                /tei         │
└─────────────────────────────────────────────────────────────────────┘
```

### 1.1 Service crates (`crates/`)

| Crate | Role |
|-------|------|
| `klams-types` | Shared serde DTOs: `Fact`, `Event`, `KnowledgeItem`, `MemoryWrite`, `SearchResult`, `HealthSnapshot`. No I/O. |
| `klams-core` | Bounded mpsc queue, worker pool, dedupe hashing, `MemoryWrite` dispatch. The async heart of the service. |
| `klams-store` | Storage adapters: `PostgresStore` (sqlx, compile-time checked), `QdrantStore` (gRPC), `TeiEmbedder` (HTTP). Each adapter is a small trait + concrete impl. |
| `klams-api` | `axum` router, bearer auth middleware, request validation, error → JSON mapping, `/healthz`, `/metrics`. |
| `klams-service` | The binary. Loads `klams.toml`, wires queue + workers + HTTP server, owns the tokio runtime. |
| `klams-client` | Typed HTTP client. Used by the viewport's Tauri backend so the desktop app and any future Rust caller share one API contract. |

### 1.2 Viewport (`viewport/`)

Independent Cargo workspace + SvelteKit project. Tauri 2 native shell
hosts a static SvelteKit bundle; the Rust side exposes a small
`#[tauri::command]` surface that delegates to `klams-client`. Built on
Linux via `cargo-xwin` targeting `x86_64-pc-windows-msvc`; ships as a
single `klams-viewport.exe` with no installer.

Runtime config (`bearer`, `service_url`) lives in
`%APPDATA%\klams\config\viewport.toml`; the bearer is stored in the
Windows Credential Manager via the `keyring` crate
(`windows-native` backend). The `--debug` CLI flag opens WebView
devtools and enables per-poll diagnostic logging to
`%TEMP%\klams-viewport.log`; otherwise the app runs quietly.

### 1.3 Stateful dependencies (Docker Compose)

[`deploy/docker-compose.yml`](../deploy/docker-compose.yml) defines three
services, all attached to a single user-defined bridge network
`klams-net` with deterministic DNS aliases (`postgres`, `qdrant`,
`tei`). The default `bridge` network is intentionally not used —
rationale in [research.md §13](../specs/001-initial-mvp/research.md#13-docker-network).

| Service | Container | Bind | Volume | Notes |
|---------|-----------|------|--------|-------|
| `postgres` | `klams-postgres` | `127.0.0.1:5432` | `${KLAMS_DATA_ROOT}/postgres` | Postgres 16. Container runs as uid 999 — never `chown -R` the data tree. |
| `qdrant` | `klams-qdrant` | `127.0.0.1:6333/6334` | `${KLAMS_DATA_ROOT}/qdrant` | Pinned via `QDRANT_IMAGE_TAG` in `compose.env`. Client/server minor versions must match. |
| `tei` | `klams-tei` | `127.0.0.1:7070` | `${KLAMS_DATA_ROOT}/tei` | Embeddings. Default is the CPU image; GPU override lives in `docker-compose.gpu.yml`. Healthcheck uses `curl`. |

## 2. Data flow

### 2.1 Write path (fact / event)

```text
controller ──HTTP POST /v1/facts──▶ klams-api
                                       │ auth + per-type validators
                                       │      + sanity rules
                                       │      + optimistic-concurrency
                                       │        (expected_version)
                                       ▼
                                    klams-core enqueue (bounded mpsc)
                                       │
                                       ▼
                                    worker pool ── dedupe hash ──▶ PostgresStore (sqlx)
                                       │                              │
                                       │                              ▼
                                       │                       persisted Fact (version++)
                                       │
                                       │ lower-trust write against a higher-trust canonical?
                                       └─▶ DissentStore.insert(payload, source, ts)
                                                 │
                                                 ▼
                                          202 Accepted { dissent_id, status: "pending" }
                                          (canonical fact unchanged;
                                           fact.dissent_count incremented by trigger)
```

Writes are durable before the queue acknowledges: facts and events are
persisted to Postgres on the worker, and only durable writes increment
the success counter (SC-004). Lower-trust writes that contradict a
canonical fact are diverted to the `dissents` table rather than
overwriting; operators resolve them via
`POST /memory/dissents/{id}/{promote|discard}`. Stale-`version` writes
return HTTP 409 with the current version in the body so retries can be
mechanical.

### 2.2 Write path (knowledge item)

```text
controller ──POST /v1/knowledge──▶ klams-api ──▶ klams-core
                                                    │
                                              worker pool
                                                    │ chunk + dedupe
                                                    ▼
                                       TeiEmbedder.embed(text)  ── HTTP ──▶ tei
                                                    │
                                                    ▼
                                       QdrantStore.upsert(vector, payload)
```

Chunks become searchable within 10 s p95 under MVP load (SC-002).

### 2.3 Read path (unified search)

```text
viewport ──invoke("search_unified", q)──▶ tauri command
                                              │ klams-client
                                              ▼
                                          klams-api  ──▶ TeiEmbedder.embed(q)
                                              │                │
                                              │                ▼
                                              ├──▶ PostgresStore.search_facts(q)
                                              ├──▶ PostgresStore.search_events(q)
                                              └──▶ QdrantStore.search(vector)
                                              ▼
                                          merged SearchResult[]  ── < 500 ms p95 (SC-003)
```

### 2.4 Health + observability

* `/healthz` returns a `HealthSnapshot` (per-dependency probe result + version).
* `/metrics` exposes Prometheus counters/histograms via `axum-prometheus`.
* Structured `tracing` logs are emitted as JSON when
  `KLAMS_LOG_FORMAT=json`; the systemd unit sets this in production.
* The viewport polls `/healthz` on an exponential backoff (capped at
  60 s) and surfaces the result in the dashboard.

### 2.5 Decay task

```text
┌──────────────────────────────────────────────────────────────┐
│ klams-service                                                │
│                                                              │
│   tokio interval (decay.task_interval_seconds, default 3600) │
│             │                                                │
│             ▼                                                │
│       DecayWorker.tick()                                     │
│             │  SELECT id, fact_type, decay_weight,           │
│             │         last_used_at FROM facts                │
│             │  LIMIT decay.batch_size (default 500)          │
│             ▼                                                │
│       per-type λ from [decay.lambda] in klams.toml           │
│             │  new_w = old_w * exp(-λ · Δt)                  │
│             ▼                                                │
│       PostgresStore.batch_update_decay(...)                  │
│             │                                                │
│             ▼                                                │
│       /memory/search ranks by decay_weight × relevance       │
└──────────────────────────────────────────────────────────────┘
```

The decay loop is in-process (no separate scheduler), bounded by
`batch_size` per tick, and idempotent — a missed tick just means
the next one covers a longer Δt. Defaults are baked into the binary;
overrides live under `[decay]` in `klams.toml`.

## 2a. Phase 2 deltas (sprint 002)

Sprint 002 (`specs/002-safety-and-write-ops/`) layers safety, drift
control, and viewport curation on top of the Phase 1 pipeline without
changing the crate boundaries:

* **Validation (FR-001..FR-007)** — per-type validators + universal
  sanity rules run inside `klams-api` before the write reaches the
  queue; malformed agent writes never touch Postgres or Qdrant
  (SC-001).
* **Dissents (FR-008..FR-013)** — new `dissents` table + 
  `dissent_count` column on `facts` + BEFORE-DELETE orphan trigger.
  Endpoints: `GET /memory/dissents`, `POST /memory/dissents/{id}/promote`,
  `POST /memory/dissents/{id}/discard`. Lower-trust contradictions
  divert to dissents instead of overwriting (SC-002).
* **Optimistic concurrency (FR-014..FR-015)** — every fact carries a
  monotonically increasing `version`; writes supply `expected_version`
  and stale writes return HTTP 409 with `current_version` (SC-003).
* **Decay-aware ranking (FR-016..FR-019)** — see §2.5; per-type λ
  values configured via `[decay.lambda]` (SC-004).
* **Viewport curation (FR-020..FR-023)** — provenance panel on every
  inspector page, edit/delete with optimistic rollback, dedicated
  `/dissents` page with diff + promote/discard, nav-bar pending-dissent
  badge (SC-005).
* **`just` inner loop (FR-024..FR-030)** — top-level `justfile` is the
  single source of developer + CI commands; `just gate` runs the
  constitution's fmt/clippy/test gate (SC-006, SC-007).

Plan and spec live at
[specs/002-safety-and-write-ops/plan.md](../specs/002-safety-and-write-ops/plan.md)
and
[specs/002-safety-and-write-ops/spec.md](../specs/002-safety-and-write-ops/spec.md).

## 2b. Phase 3 deltas (sprint 003)

Sprint 003 (`specs/003-non-agentic-writes/`) adds **non-agentic
writers** (a filesystem scanner, a systemd-state monitor) that feed
klams without an LLM in the write path, plus a deployment story to
land all three klams binaries under systemd on `kubs0`. The crate
boundaries do not change; two new binaries live under `crates/`.

```text
+----------------------+        POST /memory/knowledge/index
| klams-scanner.timer  |  --->  +-----------------+  ----------------->  +---------------+
|   (systemd OnUnit-   |        |  klams-scanner  |  POST /knowledge/    | klams-service |
|    ActiveSec=1h)     |        |   (--once run)  |  delete?source_file= |  (axum + pg + |
+----------------------+        +-----------------+  -----------------> |   qdrant)     |
                                                                         +---------------+
                                                                                 ^
+----------------------+        POST /memory/events                              |
|  klams-monitor.svc   |  --->  +----------------+   --------------------------- +
|  (Type=simple,       |        |  klams-monitor |   (Service/Execution events,
|   Restart=on-fail)   |        |   sd-poll loop |    sd-bus / systemctl is-active)
+----------------------+        +----------------+
```

* **Write paths (FR-001..FR-006)** — every write response now carries
  a `path: "canonical" | "dissent"` field (additive — flattened into
  the existing `Fact` shape so pre-sprint-003 clients ignoring unknown
  fields keep working). `MemoryPolicy` is exposed at
  `GET /memory/policy` so callers can introspect dedupe + decay rules
  without reading the TOML (SC-001, SC-005).
* **Scanner (FR-007..FR-012)** — `klams-scanner` walks `~/src` and
  `~/obsidian` (configurable), honours `.gitignore` + `.klamsignore`,
  always skips `target/`, `node_modules/`, `.git/`, chunks every file
  to ≈800 chars with 200-char overlap, and POSTs to
  `/memory/knowledge/index`. A local SQLite cursor at
  `~/.local/state/klams/scanner.sqlite` short-circuits unchanged files
  on `mtime`, then on content hash. Vanished files trigger
  `/memory/knowledge/delete?source_file=<abs>` (SC-002).
* **Monitor (FR-013..FR-016)** — `klams-monitor` polls `systemctl
  is-active <service>` for a TOML-configured list of units, diffs
  state against the last poll, and POSTs only **edge transitions**
  (`active↔inactive`, `version changed`) as `Event(category=Service)`.
  Steady-state polls emit zero traffic (SC-003).
* **systemd integration (FR-017..FR-018)** — `klams-service` runs as a
  `Type=simple` unit with `After=postgresql.service qdrant.service`;
  `klams-scanner.timer` fires the scanner hourly via a `Type=oneshot`
  unit; `klams-monitor.service` is `Type=simple` with
  `Restart=on-failure`. All three units use the same hardening profile
  (`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`). The
  `install-systemd.sh` helper is idempotent, supports `--dry-run`, and
  rotates the previous binary to `<bin>.prev` so `just rollback`
  works (SC-004).
* **ansible-k handoff (FR-019..FR-022)** — self-contained directory at
  `specs/003-non-agentic-writes/handoff/` (README + spec + api-contract
  + example script) ready to `cp -r` to
  `/home/ken/ansible-k/specs/klams-integration/`. The pinned-version
  header references `GET /healthz?contract=v1` as the drift-detection
  handshake (SC-006).
* **Backwards compatibility (FR-023)** — every sprint-001 and sprint-002
  integration test still passes against the post-sprint-003 binary;
  the `path` field is additive and `MemoryPolicy` is a new endpoint.

Plan and spec live at
[specs/003-non-agentic-writes/plan.md](../specs/003-non-agentic-writes/plan.md)
and
[specs/003-non-agentic-writes/spec.md](../specs/003-non-agentic-writes/spec.md).

## 2c. Phase 5 deltas (sprint 005 — advanced retrieval)

Sprint 005 adds **hybrid retrieval, summarization, and a unified
`/memory/context` bundler** so an agent can ask "give me the most
useful context for this query under N tokens" instead of paging
through raw rows. The wire contract lives at
[specs/005-advanced-retrieval/contracts/memory-context.openapi.yaml](../specs/005-advanced-retrieval/contracts/memory-context.openapi.yaml).

### 2c.1 Hybrid retrieval (US2)

`klams_core::hybrid` introduces two primitives:

* `StoreHybridAdapter<S: Store>` — wraps a `Store` and exposes a
  `retrieve(plan)` that over-fetches each configured `RetrievalSource`
  (`Vector`, `Fts`) by 3× and post-filters payloads against
  `RetrievalFilters` (host / type / tag / repo / file / source /
  since / until).
* `fuse(sources, FusionStrategy)` — pure rank fusion. Two strategies:
  - `Rrf { k }` — reciprocal-rank fusion (default `k=60`).
  - `Weighted { vector, fts, normalization }` — score-weighted with
    `MinMax` or `ZScore` normalization (handles constant
    distributions by collapsing to uniform contribution).

### 2c.2 Context bundler (US1)

`klams_core::context::ContextBuilder` orchestrates retrieval +
token budgeting:

1. Calls the hybrid adapter once per section (facts / knowledge /
   events).
2. Buckets returned rows by `payload.section` and fuses per-section
   with the configured `FusionStrategy`.
3. Token-counts each item via `klams_core::tokens` (currently
   `cl100k_base` via `tiktoken-rs`, with a `chars_div4` fallback
   advertised in `TokenEncoderId`).
4. Greedy-fills each section under the caller's `token_budget`,
   marking `ContextBundle.truncated = true` when the budget was hit.

`POST /memory/context` (handler at
[crates/klams-api/src/handlers/context.rs](../crates/klams-api/src/handlers/context.rs))
returns a `ContextBundle { facts, knowledge, events, total_spent,
truncated, token_encoder, sections }` with per-section
`SectionMeta { status, source, degraded_reason }`. Per-section
degradation is reported in-band; only when **every** source is
unavailable does the endpoint surface `503 Service Unavailable +
Retry-After: 5` (FR-011).

### 2c.3 Summarization (US3)

`klams_core::summarize::SummarizationTask` runs at
`[summarization].task_interval` (default 60 s), guarded by a
`tokio::sync::Mutex` so cycles never lap:

* Reads a 7-day window of events via the `EventSource` trait
  (`StoreEventSource` pages in chunks of 500, capped at 50k).
* Clusters by `(host, category, day_bucket)` and emits an
  extractive headline ("3x compile, 2x test, 1x lint") via
  `summarize::extractive::event_headline()`.
* Probes Ollama (`GET /api/tags`) at the configured `ollama_url`;
  on success, marks the summary mechanism `Llm` (the LLM call
  itself is wired through `OllamaClient::generate()`); on failure,
  records `Extractive` and the digest still ships.
* Upserts active summaries via `SummaryStore::upsert_event_summary`
  into the new `summaries` table (migration `0004_summaries.sql`).

### 2c.4 Decay-config validation (US4)

`DecayConfig::validate()` (in `klams-types/src/decay.rs`) rejects
non-finite or negative λ, zero `task_interval_seconds`, or zero
`batch_size`, naming the first offending key. The service exits
with status 2 before binding the listener if validation fails
(FR-013). On success, a single `INFO` line records the resolved
per-`FactType` λs and the `klams_decay_config_reload_total`
counter is bumped (FR-014). SIGHUP-style hot-reload is out of
scope for this sprint (D-007).

### 2c.5 Viewport context preview (US5)

A new pane at `/preview` calls `POST /memory/context` and renders
the bundle with per-section status pills, a 250 ms-debounced
token-budget slider (D-009), and a raw-vs-summarized toggle.
See [`viewport.md` §6](../specs/planning/viewport.md#6-phase-4--context-preview).

### 2c.6 Metrics added

| Metric | Type | Use |
|---|---|---|
| `klams_context_request_latency_seconds` | histogram | `/memory/context` end-to-end |
| `klams_context_section_items_total{section}` | counter | items returned per section |
| `klams_summarization_runs_total{mechanism}` | counter | `extractive` vs `llm` cycles |
| `klams_summarization_lag_seconds` | gauge | wall-clock lag of the most recent cycle |
| `klams_decay_config_reload_total` | counter | successful config loads at startup |

Plan and spec for this delta live at
[specs/005-advanced-retrieval/plan.md](../specs/005-advanced-retrieval/plan.md)
and
[specs/005-advanced-retrieval/spec.md](../specs/005-advanced-retrieval/spec.md).

## 3. Deployment topology on `kubs0`

```text
kubs0
├── /ai/klams/                              KLAMS_ROOT
│   ├── config/klams.toml                   service config (perm 0600)
│   ├── data/                               KLAMS_DATA_ROOT
│   │   ├── postgres/                       uid 999:999
│   │   ├── qdrant/
│   │   └── tei/
│   └── logs/                               optional spool
│
├── systemd
│   ├── klams-service.service               (Type=simple, After=postgresql qdrant)
│   ├── klams-scanner.service               (Type=oneshot, runs `klams-scanner --once`)
│   ├── klams-scanner.timer                 (OnBootSec=5min, OnUnitActiveSec=1h)
│   └── klams-monitor.service               (Type=simple, Restart=on-failure)
│
└── docker (compose project: klams)
    └── network: klams-net (bridge)
        ├── klams-postgres
        ├── klams-qdrant
        └── klams-tei
```

The split between **systemd-managed klams-service** and
**Compose-managed dependencies** is deliberate:

* The service is a single Rust binary with no native deps beyond
  libssl — easy to ship via `cargo build --release` + `scp`, easy to
  restart with `systemctl restart klams`.
* Postgres, Qdrant and TEI all have non-trivial image/version
  management that Compose handles cleanly via `compose.env` pins.
* This avoids a chicken-and-egg dance where the service's own
  container would need to live on `klams-net` alongside its
  dependencies; instead the service connects to `127.0.0.1:<port>`
  via the published Compose ports.

Rationale in
[research.md §3](../specs/001-initial-mvp/research.md#3-klams-service-deployment).

### 3.1 Network exposure

* `klams-service` binds `0.0.0.0:7777` (set in
  [`deploy/config/klams.example.toml`](../deploy/config/klams.example.toml)
  via `listen_addr`) so the viewport on the LAN can reach it. UFW on
  `kubs0` restricts `7777/tcp` to `192.168.1.0/24`.
* Compose dependencies are bound to `127.0.0.1` only; they are reached
  by the service over loopback and never exposed to the LAN.
* All inter-container traffic stays on the `klams-net` bridge.

### 3.2 Secrets

* Bearer token: 32-byte hex in `klams.toml` (file mode `0600`).
  Constant-time compared on every request.
* Postgres password: in `compose.env` (mode `0600`) and inlined into
  the service's `postgres.url`.
* No TLS in MVP — LAN-only deployment, see
  [research.md §7](../specs/001-initial-mvp/research.md#7-auth-model-for-mvp).

## 4. Where to look next

* End-to-end provisioning steps: [setup.md](setup.md).
* Day-to-day operator recipes (start/stop, log inspection, viewport
  install): [usage.md](usage.md).
* MVP smoke checks mapped to success criteria:
  [specs/001-initial-mvp/quickstart.md §9](../specs/001-initial-mvp/quickstart.md#9-smoke-test-the-user-stories).
* Per-decision rationale:
  [specs/001-initial-mvp/research.md](../specs/001-initial-mvp/research.md).
