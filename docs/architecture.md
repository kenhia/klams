# klams Architecture

This document describes how the klams MVP is assembled at runtime: which
components exist, how they communicate, where their state lives, and how
they are deployed on the production host `kubs0`. It complements
[setup.md](setup.md) (provisioning) and
[usage.md](usage.md) (operator-facing recipes), and is the operator-
oriented counterpart to the formal design records in
[sprints/001-initial-mvp/plan.md](../sprints/001-initial-mvp/plan.md) and
[sprints/001-initial-mvp/research.md](../sprints/001-initial-mvp/research.md).

## 1. Components

```text
                       ┌─────────────────────────────┐
                       │  klams-viewport             │
                       │  (Windows / Linux / WSL)    │
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
│   │   ├───────────────────────────────────────────────────────┤  │  │
│   │   │ Backup task   tokio scheduler → pg_dump + qdrant      │  │  │
│   │   │               snapshot → retention; flips             │  │  │
│   │   │               MaintenanceState + fires status_hook    │  │  │
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

### 1.3 Stateful dependencies (Docker Compose)

[`deploy/docker-compose.yml`](../deploy/docker-compose.yml) defines three
services, all attached to a single user-defined bridge network
`klams-net` with deterministic DNS aliases (`postgres`, `qdrant`,
`tei`). The default `bridge` network is intentionally not used —
rationale in [research.md §13](../sprints/001-initial-mvp/research.md#13-docker-network).

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

Since sprint 027 the handler gates `text` against the embedder's token
ceiling **before** enqueueing, so a chunk the model would refuse gets a
`413` instead of a `202`. That ordering is the point: the scanner
advances its cursor on the `202`, and the worker has no reply channel, so
anything accepted here and rejected later is lost silently (see §2m).

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

Sprint 002 (`sprints/002-safety-and-write-ops/`) layers safety, drift
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
[sprints/002-safety-and-write-ops/plan.md](../sprints/002-safety-and-write-ops/plan.md)
and
[sprints/002-safety-and-write-ops/spec.md](../sprints/002-safety-and-write-ops/spec.md).

## 2b. Phase 3 deltas (sprint 003)

Sprint 003 (`sprints/003-non-agentic-writes/`) adds **non-agentic
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
  always skips `target/`, `node_modules/`, `.git/`, and (sprint 021)
  applies a file-type allowlist so only content worth retrieving —
  source, docs/prose, config prose — is indexed; lockfiles, JSON
  fixtures, SVGs, and images are dropped. Chunking is **language-aware**
  (sprint 022): markdown splits on headings with heading-*path* context
  (no bare-heading chunks); Rust/Python parse with tree-sitter and split
  at item boundaries carrying symbol names; everything else splits on
  blank lines (a `#` comment is never a heading). Chunks carry
  index/language/heading-path/symbol metadata to the payload, and are
  POSTed to `/memory/knowledge/index`. A local SQLite cursor at
  `~/.local/state/klams/scanner.sqlite` short-circuits unchanged files
  on `mtime`, then on content hash. Vanished files trigger
  `/memory/knowledge/delete?source_file=<abs>` (SC-002); since sprint
  021 a *changed* file triggers the same delete **before** its new
  chunks are published, so edits replace rather than accumulate stale
  points.
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
  (`NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`). Note
  `ProtectSystem=strict` makes the filesystem read-only for the
  service: any writable path outside `StateDirectory` needs an
  explicit `ReadWritePaths=` (the backup dir gained one in sprint 020
  after the hardened unit silently broke nightly backups for 40
  days). The
  `install-systemd.sh` helper is idempotent, supports `--dry-run`, and
  rotates the previous binary to `<bin>.prev` so `just rollback`
  works (SC-004).
* **ansible-k handoff (FR-019..FR-022)** — self-contained directory at
  `sprints/003-non-agentic-writes/handoff/` (README + spec + api-contract
  + example script) ready to `cp -r` to
  `/home/ken/ansible-k/specs/klams-integration/`. The pinned-version
  header references `GET /healthz?contract=v1` as the drift-detection
  handshake (SC-006).
* **Backwards compatibility (FR-023)** — every sprint-001 and sprint-002
  integration test still passes against the post-sprint-003 binary;
  the `path` field is additive and `MemoryPolicy` is a new endpoint.

Plan and spec live at
[sprints/003-non-agentic-writes/plan.md](../sprints/003-non-agentic-writes/plan.md)
and
[sprints/003-non-agentic-writes/spec.md](../sprints/003-non-agentic-writes/spec.md).

### Sprint 010 — ingestion operationalized

Sprint 010 (`sprints/010-operationalize-ingestion/`) takes the sprint-003
scanner and monitor from "buildable" to **live on `kubs0`**:

* `klams-scanner.timer` fires the scanner hourly (`Type=oneshot`); the
  scanner walks `/home/ken/src` + `/home/ken/obsidian`, pruning heavy
  dependency/cache trees (`target`, `node_modules`, `.pnpm-store`,
  `.venv`, `__pycache__`, `.obsidian`, …) **before** descent and
  honouring `.gitignore` + a repo-root `.klamsignore`. End-to-end
  ingestion (index, attribution, ignore-handling, idempotency, delete-
  on-vanish) is verified by sentinel-note acceptance probes.
* `klams-monitor.service` (`Type=simple`) polls `klams-service.service`
  and posts typed `Service` events on edge transitions. **Known
  limitation:** because the monitor posts events to `klams-service`
  itself, it cannot record `klams-service`'s own *Down* (the sink is
  unavailable during the outage) — a documented known limitation
  (kwi #55); the outage stays reconstructable from the gap to the
  recovery *Up*. Service events now carry the real host (read from
  `/proc/sys/kernel/hostname`), not `host=unknown` (kwi #56, fixed).
* The units declare `After=/Wants=docker.service` — Postgres, Qdrant,
  and the TEI embedder run in Docker; there is no host
  `postgresql.service`.
* The legacy python looper (`~/src/tools/ksvc-looper/klams_monitor.py`)
  is a **kpidash app-health reporter** (polls `/healthz` → dashboard),
  not a klams event source, so it is *not* replaced by the Rust monitor's
  event path — the two observe different signals. That kpidash path is
  now **re-homed into the Rust monitor** behind the default-on `kpidash`
  cargo feature: an optional `[kpidash]` config section makes
  `klams-monitor` poll `/healthz` and publish the same
  `kpidash:services:<name>:<host>` Redis card the looper wrote (identical
  `{ts,state,text,host,icon}` JSON). It lives in the monitor — not the
  service — so it stays an *external* observer that can still report
  `down` when `klams-service` is offline (the kpidash Redis sink is on a
  separate host, side-stepping the kwi #55 self-dependency). The whole
  section is inert when omitted, so a clone without Redis never connects.
  See [`crates/klams-monitor/src/kpidash.rs`](../crates/klams-monitor/src/kpidash.rs).
  Live cutover (stop the looper, deploy the configured monitor) is the
  remaining operator step.

## 2c. Phase 5 deltas (sprint 005 — advanced retrieval)

Sprint 005 adds **hybrid retrieval, summarization, and a unified
`/memory/context` bundler** so an agent can ask "give me the most
useful context for this query under N tokens" instead of paging
through raw rows. The wire contract lives at
[sprints/005-advanced-retrieval/contracts/memory-context.openapi.yaml](../sprints/005-advanced-retrieval/contracts/memory-context.openapi.yaml).

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
* Probes the configured OpenAI-compat chat endpoint
  (`GET {llm_url}/models`); on success, marks the summary mechanism
  `Llm` (the LLM call itself is wired through
  `OpenAiChatClient::generate()` — sprint 014, previously
  Ollama-native); on failure, records `Extractive` and the digest
  still ships.
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
See [`viewport.md` §6](../sprints/planning/viewport.md#6-phase-4--context-preview).

### 2c.6 Metrics added

| Metric | Type | Use |
|---|---|---|
| `klams_retrieval_duration_seconds{op, transport}` | summary | search + context latency at every entry point (REST + MCP; sprint 020 replaces the context-only histogram) |
| `klams_context_section_items_total{section}` | counter | items returned per section |
| `klams_summarization_runs_total{mechanism}` | counter | `extractive` vs `llm` cycles |
| `klams_summarization_lag_seconds` | gauge | wall-clock lag of the most recent cycle |
| `klams_decay_config_reload_total` | counter | successful config loads at startup |

Plan and spec for this delta live at
[sprints/005-advanced-retrieval/plan.md](../sprints/005-advanced-retrieval/plan.md)
and
[sprints/005-advanced-retrieval/spec.md](../sprints/005-advanced-retrieval/spec.md).

## 2d. Phase 6 deltas (sprint 006 — maintenance & backups)

Sprint 006 adds an in-process **backup task** to `klams-service` that
takes a Postgres `pg_dump` and a Qdrant snapshot once per UTC day, an
axum middleware that quiesces non-critical writes while a backup is
in flight, a generic exec-with-JSON status hook so external observers
(kpidash, ansible-k) can subscribe to the lifecycle, and a Grafana
dashboard authored alongside the metrics. The crate boundaries do
not change.

```text
┌───────────────────────────────────────────────────────────────────┐
│ klams-service                                                     │
│                                                                   │
│   ┌──────────────┐  every UTC day  ┌────────────────────────────┐ │
│   │  scheduler   │  ─────────────▶ │  Backup task               │ │
│   │ (sleep_until │                 │   1. mark MaintenanceState │ │
│   │  window_utc) │                 │      .active = true        │ │
│   └──────────────┘                 │   2. status_hook "started" │ │
│                                    │   3. pg_dump  -> *.partial │ │
│                                    │      atomic rename         │ │
│                                    │   4. qdrant snapshot ditto │ │
│                                    │   5. retention prune       │ │
│                                    │   6. status_hook           │ │
│                                    │      "finished" / "failed" │ │
│                                    │   7. clear MaintenanceState│ │
│                                    └─────────────┬──────────────┘ │
│                                                  │                │
│                              ┌───────────────────┼──────────────┐ │
│                              ▼                   ▼              ▼ │
│                         pg_dump 16          qdrant REST   status_hook │
│                         (TCP 5432)         (HTTP 6333)    (exec + stdin JSON) │
│                                                                   │
│   axum router ── maintenance_check middleware ──▶ 503 + Retry-After│
│                  (reads MaintenanceState.active)                  │
│                                                                   │
│   /healthz ──▶ HealthSnapshot.maintenance { active, run_id, ...} │
└───────────────────────────────────────────────────────────────────┘
                          │
                          ▼
              ${backup_dir}/
              ├── lockfile                       (pid + run_id; stale-recovery on startup)
              ├── postgres-YYYY-MM-DD.dump       (atomic; .partial during write)
              ├── qdrant-YYYY-MM-DD.snapshot
              └── (older dates pruned per daily_count + weekly_count)
```

* **Scheduler (FR-001..FR-006)** — hand-rolled `tokio::time::sleep_until`
  loop on the next UTC `[backup].window_start_utc` instant, calling
  `backup::run_once` exactly once per day. Skipped with a single INFO
  line when `[backup].enabled = false`.
* **Orchestrator** — `klams_service::backup::run_once` flips
  `MaintenanceState::active = true`, fires the `started` hook, takes
  the Postgres dump then the Qdrant snapshot then the retention prune,
  fires `finished` / `failed`, and clears the flag in a guard that
  runs on both success and failure paths. Lockfile + `.partial` files
  protect against mid-run crashes — startup recovery cleans both up
  and emits a `failed` hook with `error: "service_restarted_mid_backup"`.
* **MaintenanceState (FR-007..FR-008)** — shared `Arc<RwLock>` in
  `klams-types` so `klams-api` can depend on it without dragging in
  `klams-service`. The `maintenance_check` middleware short-circuits
  non-`GET`, non-critical-write requests with a `503 + Retry-After`
  envelope when active; the critical-write set (currently
  `POST /memory/dissents/{id}/{promote,discard}`) is matched
  path-wise because axum global layers run before routing.
* **Hook executor (FR-009..FR-012)** — `tokio::process::Command` with
  piped stdin, env passthrough for `KLAMS_BACKUP_RUN_ID` +
  `KLAMS_BACKUP_EVENT`, bounded by `status_hook_timeout` with a 2s
  SIGTERM grace before SIGKILL (via `nix::sys::signal`, since
  `unsafe_code = forbid` rules out `libc::kill` directly). Hook
  failure is observability, not control flow — see
  `crates/klams-service/src/backup/hook.rs`. Schema:
  [`contracts/backup-status-hook.schema.json`](../sprints/006-maintenance-and-backups/contracts/backup-status-hook.schema.json).
* **Retention (FR-005)** — filename-as-truth date parsing keeps the
  newest `daily_count` distinct dates + the newest `weekly_count`
  Sundays per kind; treats `same_day_strategy = "suffix"` runs as the
  same date for retention (highest-N copy wins). No mtime consulted.
* **Metrics** — five new Prometheus series:
  `klams_backup_runs_total{ok}`,
  `klams_backup_duration_seconds{kind}`,
  `klams_backup_last_success_timestamp_seconds`,
  `klams_backup_size_bytes{kind}`,
  `klams_backup_hook_invocations_total{event,ok}`.
* **Grafana dashboard (US5)** —
  [`deploy/grafana/klams.json`](../deploy/grafana/klams.json) ships
  the 11-panel dashboard (queue / throughput / latency / errors /
  backup age / maintenance / summarization / backup duration / runs
  by ok / hook invocations). **Production install lives in
  ansible-k, not here.** The handoff document at
  [`~/ansible-k/specs/klams-integration/klams-grafana.md`](../../../ansible-k/specs/klams-integration/klams-grafana.md)
  enumerates every series the panels consume and the two recommended
  alerts (`klams_backup_stale`, `klams_backup_failures`); the
  `tests/grafana_dashboard_json.rs` integration test parses both and
  fails if the dashboard references a series the handoff does not
  list. SC-008's cross-link assertion is satisfied by this paragraph.

Plan and spec live at
[sprints/006-maintenance-and-backups/plan.md](../sprints/006-maintenance-and-backups/plan.md)
and
[sprints/006-maintenance-and-backups/spec.md](../sprints/006-maintenance-and-backups/spec.md).

## 2e. Phase 7 deltas (sprint 007 — MCP projection layer)

Sprint 007 (`sprints/007-mcp-server/`) exposes klams over the **Model
Context Protocol** without changing the underlying Postgres/Qdrant
schemas. The MCP surface is a new public projection on top of the
existing stores; everything below it (decay, dissents, dedupe,
embedding pipeline) is untouched.

### 2e.1 `authors` table — first-class attribution

New table `authors` (migration `0005_authors_table.sql`) attributes
every memory to the agent that wrote it. `facts.author_id` and
`events.author_id` become NOT NULL FKs after a backfill to the
seeded `SYSTEM_AUTHOR_ID` (`00000000-0000-7000-8000-000000000001`);
Qdrant points carry `author_id` in payload. Schema reference:
[sprints/007-mcp-server/data-model.md §1–§4](../sprints/007-mcp-server/data-model.md).

Authors are registered via the `register_author` MCP tool, though
since sprint 018 a caller rarely needs it: the author bound to the
bearer token (`agent_name` in `[[auth.tokens]]`) attributes writes
automatically. The server bumps `last_seen_at` on each touch
(FR-005).

Sprint 025 (#636) gave the registry the verbs it had been missing —
it could only ever *create*. `register_author` now dedupes on
`agent_name` (it minted a fresh UUIDv7 per call, which is how the
store reached 8 `klams-mind` and 6 `kyac` rows), and the
`memory_admin_{list,remove,merge}_author*` tools cover inspection,
block-if-owned removal, and transactional merge. Removal refuses
while an author owns facts, events, knowledge points, or recorded
soft-deletes; reassigning is `merge`'s job and is explicit.

### 2e.2 Public projection (`PublicMemory`)

The only shape returned by MCP tools and the viewport author REST
endpoints is `klams_types::PublicMemory`. The internal `Fact`,
`Event`, and `KnowledgeItem` types are **never** serialized across
the public boundary; the projection deliberately omits
`version`, `decay_weight`, `confidence`, `use_count`, `last_used_at`,
raw embedding vectors, and the internal `source` trust tier
(`User`/`Controller`/`Task`/`AgentProposal`).

```text
+-----------------------+   Streamable    +---------------------+
| MCP client            |   HTTP / SSE    |  klams-mcp          |
| (VS Code, Copilot     |  ◄ JSON-RPC ──► |  rmcp ServerHandler |
|  CLI, custom)         |   over POST/GET |  + scope filter     |
+-----------------------+                 +---------+-----------+
                                                    │
                                                    ▼
                                          +---------------------+
                                          |  ProjectionLayer    |
                                          | (Fact|Event|Knowledge|
                                          |   → PublicMemory)   |
                                          +----+---------+------+
                                               │         │
                              +----------------+         +-------------+
                              ▼                                        ▼
                    +--------------------+                  +--------------------+
                    |  PostgresStore     |                  |  QdrantStore       |
                    | (facts, events,    |                  | (knowledge_items,  |
                    |  authors)          |                  |  authors-aware     |
                    +--------------------+                  |   payload)         |
                                                            +--------------------+
```

### 2e.3 Scope-gated tool surface

Every MCP tool is gated by a `Scope` checked from the bearer token's
`TokenGrant`. Sprint 025 added a fourth tier, `Manage`, for
cross-author curation. The legacy single `bearer_token` field is
materialized at load time into one grant with all four scopes; the
`[[auth.tokens]]` array (see
[data-model.md §5](../sprints/007-mcp-server/data-model.md#5-configuration-extension-klams-typesauthconfig))
issues per-purpose tokens. Insufficient-scope calls return a
deterministic `permission_denied` error; scope failures are counted
by `klams_mcp_scope_denied_total{scope,tool}`.

**Scopes are flat, not hierarchical** — `Scope::satisfies` is exact
equality, so `Write` does not imply `Read` and `Admin` does not imply
`Manage`. Every grant lists what it needs. This is deliberate:
granting a broad-sounding scope can never silently confer a
capability that was not intended.

| Tool family | Scope |
|-------------|-------|
| `memory_search`, `memory_related`, `event_search` | `Read` |
| `memory_add`, `memory_append_event`, `memory_delete`, `dissent_propose`, `register_author` | `Write` |
| `memory_admin_*` (restore, hard_delete, list_deleted, list/remove/merge authors) | `Admin` |

`Manage` gates behaviour rather than whole tools: `memory_delete`
requires it to delete a memory authored by *somebody else*
(self-management needs only `Write`), and on the REST surface it
gates dissent promote/discard. `register_author` moved `Read` →
`Write` in sprint 025 — minting identities was a read-scope operation
until then.

Sprint 025 also layered `require_scope` onto **every** protected REST
route. Before that it was installed on exactly one (`/v1/memories`),
so the `scopes` list in `[[auth.tokens]]` was decorative on that
surface: any valid bearer could index knowledge, bulk-delete it, and
resolve dissents. Full model: [auth.md](auth.md).

### 2e.4 Soft-delete representation

Facts and knowledge items support **soft delete**:
`deleted_at` (timestamptz / Qdrant payload string) is NULL for live
rows, set to the UTC delete time for tombstoned ones. Every read
path applies `WHERE deleted_at IS NULL` (or the Qdrant equivalent
`must_not deleted_at`) unless an admin tool explicitly asks for the
inverse. `events` are append-only and **never** carry soft-delete
columns (FR-015).

| State | Postgres | Qdrant payload | Visible to `memory_search` | Visible to `memory_admin_list_deleted` |
|-------|----------|----------------|----------------------------|----------------------------------------|
| live | `deleted_at IS NULL` | no `deleted_at` key | yes | no |
| soft-deleted | `deleted_at = T`, `deleted_by_author_id = A` | `deleted_at = T`, `deleted_by_author_id = A` | no | yes |
| hard-deleted | row removed | point removed | no | no |

The viewport drilldown at `/authors/{id}` consumes the same
projection via `GET /v1/authors/{id}/memories` and renders a state
badge (`live` | `soft-deleted` | `hard-deleted`) plus a
cross-kind link `{id, kind} → /facts|/knowledge|/events/{id}`
(FR-025).

### 2e.5 HTTP transport & auth wiring

The MCP surface is mounted at `/mcp` on the same axum router as the
REST API (no separate listener). `klams-service::main` builds:

```text
Router::new()
  .merge(protected_rest)      // /v1/* behind require_bearer
  .merge(public)              // /healthz, /metrics (when enabled)
  .nest("/mcp",
        klams_mcp::router(mcp_state, cfg.server.mcp_allowed_hosts)
            .layer(require_bearer))   // ← layer attached HERE
```

The `require_bearer` layer **must** wrap the `/mcp` sub-router
directly; layering on the outer `Router::new()` after `.nest(...)`
does not apply to nested services in axum 0.8. The shared
`AuthState` (built from `[auth.bearer_token]` + `[[auth.tokens]]`)
backs both the REST and MCP gates so a single token works for both.

`klams_mcp::router` wraps rmcp's `StreamableHttpService` with a
configurable Host-header allowlist (`[server].mcp_allowed_hosts`).
Default is empty — the allowlist is **disabled** because
`require_bearer` is the real access control; bearer-less requests
are rejected with `401` before any tool sees them. Operators who
want DNS-rebinding belt-and-suspenders can set the list explicitly
(e.g. `["localhost", "workstation:7777"]`).

No OAuth metadata is served. VS Code Insiders' `"type": "http"`
client accepts a static `headers.Authorization` in `mcp.json` and
treats the absent `/.well-known/oauth-protected-resource` as a
harmless warning. The handshake walkthrough lives at
[sprints/007-mcp-server/research-vscode-mcp-http.md](../sprints/007-mcp-server/research-vscode-mcp-http.md).

Plan and spec live at
[sprints/007-mcp-server/plan.md](../sprints/007-mcp-server/plan.md)
and
[sprints/007-mcp-server/spec.md](../sprints/007-mcp-server/spec.md).

## 2f. Phase 8 deltas (sprint 008 — Activity observability)

Sprint 008 (`sprints/008-activity-observability/`) closes the
observability triangle around the MCP layer added in sprint 007: one
agent-facing tool, one operator-facing HTTP surface, and one shared
viewport tab — all reading from the **same query path** so the
numbers agents see and the numbers operators see can never diverge.
No new storage; no schema changes.

### 2f.1 Shared query layer

A single `Store::list_memories` method on the
[klams-store](../crates/klams-store/src/lib.rs) trait projects
`facts`, `events` and `knowledge` rows into a uniform `PublicMemory`
stream, globally newest-first: each kind is paged `created_at DESC`
after one shared `(created_at, id)` keyset and merged, behind an opaque
cursor (kwi #54 — knowledge, in Qdrant, is ordered via a `created_at`
datetime index rather than point-id order). Both `event_search` (MCP)
and `GET /v1/memories` (HTTP) delegate to it; there is no parallel SQL
anywhere. Rationale (R-001 — "two
surfaces, one query") lives in
[sprints/008-activity-observability/research.md](../sprints/008-activity-observability/research.md).

```text
   MCP event_search        HTTP GET /v1/memories       Viewport /activity
          │                          │                          │
          └────────────┬─────────────┴─────────────┬────────────┘
                       ▼                           ▼
              klams-store::list_memories     klams-store::event_search
                       │                           │
                       └─────────── pure SQL ──────┘   (no embedding call)
```

`event_search` is **pure-SQL on the events table** — it never invokes
the embedder (FR-004); the `tei_requests_total` counter must not
increment for a search-only workload. This is the contract that
agents can rely on for cheap event lookup.

### 2f.2 Operator surface — `GET /v1/memories`

New read-only route on `klams-api` returning the same `PublicMemory`
projection with bearer scope `read`. Defaults to a 24-hour window;
windows larger than 30 days return HTTP 400 `WINDOW_TOO_LARGE`. Soft-
deleted rows are surfaced via `state=deleted` with the original
`deleted_at` / `deleted_by` metadata preserved (FR-015a) so operators
can inspect what was removed without restoring it.

### 2f.3 Viewport — `/activity` tab

New SvelteKit route at
[viewport/src/routes/activity/+page.svelte](../viewport/src/routes/activity/+page.svelte)
wraps `GET /v1/memories` via a `viewport_list_memories` Tauri command.
Filters: time window, kinds, authors, live / soft-deleted / all. Rows
link to the per-kind detail page regardless of state so soft-deleted
items remain navigable.

### 2f.4 Grafana panel fix — author activity

Sprint 007 shipped three MCP author counters
(`klams_mcp_writes_total`, `klams_mcp_deletes_total`,
`klams_mcp_search_total`) but no dashboard panels for them, leaving
SC-005 ("operator can see per-author MCP activity") un-met. Sprint 008
adds three panels to
[deploy/grafana/klams.json](../deploy/grafana/klams.json) (writes /
deletes / search by `agent_name`), wires
[deploy/prometheus/prometheus.yml](../deploy/prometheus/prometheus.yml)
to scrape `klams-service:7777/metrics`, and gates both behind the
existing `observability` Compose profile so the production stack is
unaffected when the profile is not selected. The handoff table in
ansible-k's
[`sprints/klams-integration/klams-grafana.md`](https://github.com/kenhia/ansible-k/blob/main/specs/klams-integration/klams-grafana.md)
gained matching rows for the three series — the
`every_panel_series_appears_in_handoff_table` contract test enforces
this going forward.

### 2f.5 Performance baseline harness — `klams-bench`

Non-shipping crate at [tools/bench/](../tools/bench/) with two
binaries: `seed` (writes a deterministic
`ChaCha20Rng::from_seed`-generated corpus via the existing write
surfaces, with 503/queue-full exponential-backoff retry) and `run`
(replays a representative query set against `memory_search`, records
microsecond latencies into an HDR histogram, writes
[sprints/008-activity-observability/perf-baseline.md](../sprints/008-activity-observability/perf-baseline.md)).
Per FR-022 the harness **never gates `just gate`** — `bench-seed` and
`bench-run` always exit 0; the artifact is a measurement, not an
assertion. The baseline file auto-tags "Smoke run" when the corpus is
below the canonical 10k facts / 50k knowledge target.

Plan and spec live at
[sprints/008-activity-observability/plan.md](../sprints/008-activity-observability/plan.md)
and
[sprints/008-activity-observability/spec.md](../sprints/008-activity-observability/spec.md).

## 2g. Phase 9 deltas (sprint 009 — Stability & attribution)

Sprint 009 (`sprints/009-stability-attribution/`) closes three
production wounds left open after sprint 008: the loopback CLOSE_WAIT
leak that exhausted file descriptors under sustained traffic
(kwi #26), the REST attribution gap that stamped every non-MCP write
as `system` (kwi #28), and a viewport drilldown 404 from the Authors
view. No new storage, no schema changes — every fix is in the
service plumbing.

### 2g.1 Connection-limits layer

A per-peer `ConnectionLimits` tower layer wraps the axum service in
[`klams-service::main`](../crates/klams-service/src/main.rs). The
layer caps concurrent in-flight requests per remote IP and trims
idle keep-alive connections so a misbehaving client (or a long-lived
loopback writer that fails to close) cannot accumulate sockets in
`CLOSE_WAIT` indefinitely. The packaged systemd unit raises
`LimitNOFILE=65536` (see [deploy/klams-service.service](../deploy/klams-service.service))
so the in-app cap is reached before the kernel-level fd cap is hit.
Validated by an 18-hour loopback soak harness exposed as
`just soak --duration 18h` ([tools/soak/](../tools/soak/)).

### 2g.2 Attribution flow — bearer → author_id

```text
client HTTP request                 service startup
   Authorization: Bearer <tok>          │
              │                          ▼
              ▼                  [auth.tokens] grants
     require_bearer middleware           │ each grant
   resolves token → TokenGrant           ▼ with agent_name
              │                  AuthorBinding cache
              ▼                  agent_name → author_id
     Request::extensions.insert(             (one row in
          AuthorBinding { author_id })       authors table)
              │
              ▼
     REST handler extracts AuthorBinding
              │
              ▼
     UpsertFact { author_id, .. } → worker
     AppendEvent { author_id, .. } → worker
     IndexKnowledge { author_id, .. } → worker
              │
              ▼
     PostgresStore::upsert_fact_with_author(... author_id ...)
     QdrantStore::index_knowledge_with_author(... author_id ...)
```

Each `[[auth.tokens]]` entry now carries an optional `agent_name`
([crates/klams-types/src/auth.rs](../crates/klams-types/src/auth.rs))
validated at startup against a strict charset (lowercase, digits,
`-`/`_`) so a typo never reaches the cache. Tokens without
`agent_name` fall back to `system`. The legacy `bearer_token` field
is materialized as a `system`-bound grant, so existing deployments
keep working with no config change. Multiple tokens may share an
`agent_name`; they all resolve to the same `author_id`.

### 2g.3 One-shot re-attribution repair

For deployments with historical `system`-stamped REST writes,
[tools/reattribute-system/](../tools/reattribute-system/) ships a
standalone CLI that walks `facts`/`events`/`knowledge_items`, finds
the `register_author` event that immediately preceded each write,
and reassigns the row to that author. Rows with no resolvable
antecedent land on the new `lost-author` seed identity rather than
staying on `system`, keeping the bucket sum invariant intact. The
repair is idempotent and dry-run by default; `--apply` commits. The
store-level invariant tests live in [crates/klams-store/src/repair.rs](../crates/klams-store/src/repair.rs).

### 2g.4 Test isolation

The Phase 6 MCP test harness ([crates/klams-service/tests/common/mod.rs](../crates/klams-service/tests/common/mod.rs))
gained `TestServer::spawn_isolated()`: each test gets a per-test
Qdrant collection (`klams_test_{uuid}`) and a truncated Postgres
between runs, with the seeded `system` and `lost-author` identities
preserved so attribution invariants still hold. Validated 10/10
under default parallelism.

Plan and spec live at
[sprints/009-stability-attribution/plan.md](../sprints/009-stability-attribution/plan.md)
and
[sprints/009-stability-attribution/spec.md](../sprints/009-stability-attribution/spec.md).

## 2h. Sprint 014 deltas — serving pivot

Sprint 014 ([sprints/014-serving-pivot/](../sprints/014-serving-pivot/sprint.md))
decouples klams from vendor-native model-serving APIs so the engine
behind embeddings and summarization is a **configuration choice**, not
a build choice. No schema changes; no new storage.

* **`Embedder` trait** ([crates/klams-store/src/embeddings.rs](../crates/klams-store/src/embeddings.rs))
  — `CompositeStore.embedder` is now `Arc<dyn Embedder>`. Two impls:
  `TeiEmbedder` (TEI-native `POST /embed`, the production default) and
  `OpenAiCompatEmbedder` (`POST {url}/embeddings`, OpenAI shapes,
  optional bearer key). Selected via `[embeddings] api = "tei" | "openai"`;
  for `openai` the `url` must include the version segment (TEI's own
  `/v1` route, vLLM, and Ollama `/v1` all work).
* **Chat client** ([crates/klams-core/src/summarize/llm.rs](../crates/klams-core/src/summarize/llm.rs))
  — `OpenAiChatClient` (`GET {llm_url}/models` probe,
  `POST {llm_url}/chat/completions`) replaces the Ollama-native
  client. Config: `[summarization] llm_url` / `llm_model` /
  `llm_api_key`; the legacy `ollama_url` / `ollama_model` keys parse
  as serde aliases, but the URL value must now include `/v1`.
* **Embedding topology decision** — embeddings stay local to `kubs0`
  (TEI container, GPU-capable); the OpenAI-compat path exists so a
  future switch to vLLM/kvllm is a URL change. `vector_dim` remains
  config-driven end-to-end (config → Qdrant collection bootstrap →
  per-embed `expected_dim` check); changing the embedding model is a
  deliberate re-embed event — procedure in
  [sprints/014-serving-pivot/re-embed-runbook.md](../sprints/014-serving-pivot/re-embed-runbook.md).

## 2i. Sprint 015 deltas — companion enablement

Sprint 015 ([sprints/015-companion-enablement/](../sprints/015-companion-enablement/sprint.md))
onboards `klams-mind` (the Python/LangChain companion at
`~/src/ai/klams-mind`) as a first-class, attributable agent.

* **`dissent_propose` MCP tool** (`Write` scope) — file a dissent
  directly against a live canonical fact: proposed correction +
  required `reason`, optional `contradicting_memory_id`. This is the
  external path for *semantic* contradiction detection; the write-path
  trigger (§2.1) still handles same-fact trust conflicts. Proposals
  land as `Source::AgentProposal`, reuse the pending
  `(fact_id, payload_hash)` dedupe, and resolve only through the
  viewport `/dissents` promote/discard flow. Migration
  `0009_dissent_proposals.sql` adds nullable `reason` /
  `contradicting_memory_id` / `author_id` provenance columns;
  write-path dissents leave them NULL.
* **Viewport detail routes** (kwi #31) — `/facts/{id}`, `/events/{id}`,
  `/knowledge/{id}` pages now exist (the `hrefFor()` links from the
  Activity/Authors views previously 404'd); a vitest route-existence
  guard prevents future dangling links. Fact details link to their
  pending dissents.
* **Token grant** — the example config gains a commented
  `klams-mind` read+write `[[auth.tokens]]` block with
  `agent_name = "klams-mind"`.
* **Surface split (binding)** — the agent surface is MCP-only
  (`PublicMemory` projection); REST is the controller/operator surface.
  klams-mind uses REST only for `GET /v1/memories` bulk reads and
  `/healthz`.

## 2j. Sprint 016 deltas — retrieval diagnostics

Sprint 016 ([sprints/016-retrieval-diagnostics/](../sprints/016-retrieval-diagnostics/sprint.md))
makes `memory_search` results diagnosable for klams-mind's retrieval
eval harness. No storage or schema change.

* **`memory_search` now returns `klams_types::ScoredMemory` envelopes**
  (`{ score, source_rank, memory }`) instead of bare `PublicMemory`.
  The tool previously computed a per-hit relevance score and discarded
  it; now it surfaces `score` and `source_rank` (the hit's 0-based rank
  within its own source's result list, before cross-source fusion — the
  global rank is the array index). The wrapped `memory` is unchanged;
  its `kind` doubles as the source discriminator, so there is no
  separate `source_kind`. Distinct from the REST `/memory/search`
  `SearchHit` (a flattened preview/payload shape), which is untouched.
* **Known limitation — cross-kind score scale.** The merged sort mixes
  Qdrant cosine similarity (knowledge, ~0..1) with Postgres `ts_rank`
  (facts/events, unbounded, typically ≪1), biasing the order toward
  knowledge. Sprint 016 *exposes* this via `score` + `memory.kind`
  rather than correcting it (roadmap item-2 / YAGNI — no failing eval
  metric demands a fusion fix yet). Compare scores only within a kind.
* **Consumer**: klams-mind's client + eval runner update in lockstep;
  the contract change is recorded in that repo at
  `sprints/planning/001-cross-project-note.md`.

## 2k. Sprint 026 deltas — query-time dedupe + projection additions

Sprint 026 ([sprints/026-retrieval-measurement/](../sprints/026-retrieval-measurement/sprint.md),
WI #641) collapses duplicate content at query time and widens the
knowledge projection. No storage or schema change, no migration.

* **Why**: sprint 023 deliberately made *ingest* dedupe host-scoped
  (`find_knowledge_by_content_hash(hash, file, machine)`) so that
  per-host delete works. `kai` and `kubs0` scan a synced `~/src`, so
  nearly every chunk is stored twice. Measured on the live corpus:
  221,982 points, 124,034 unique `content_hash`, 36,121 hashes on both
  hosts — and a 10-result page was reliably 5 duplicate pairs.
* **Query-time collapse** (`klams_core::dedupe::collapse_duplicates`)
  groups knowledge hits by `content_hash` and keeps the best-ranked
  copy. It runs **before** rank fusion, so freed ranks compact and the
  released slots fill with new content instead of leaving holes. The
  fetch over-fetches (MCP ×2, the hybrid adapter's existing ×3) so a
  page stays full after collapse.
  - Keys on **content only** — host/file/repo never keep two copies
    apart. Duplicate storage was never the problem; duplicate top-k
    slots were.
  - The survivor carries `copies: [{id, host, file}]` for every
    duplicate it absorbed, so nothing becomes unreachable.
  - **Top-k scope**: a copy outside the fetch window is not hunted down.
  - Applied on all three read paths: MCP `memory_search`, REST
    `/memory/search`, and `/memory/context` (the latter two share the
    `StoreHybridAdapter` seam). Facts and events carry no
    `content_hash` and are never collapsed. Distinct from the
    `ContextBuilder`'s cross-section dedupe (repo+file), which is
    unchanged.
* **Projection additions** on knowledge results:
  `content_hash` (the collapse key, and the eval's no-duplicates
  assertion), `heading_path` / `language` / `chunk_index`, and
  `author.id` (ownership reasoning for delete/supersede without a
  `register_author` round-trip).
* **`KnowledgeItem` gained `heading_path` / `language` / `chunk_index`.**
  Sprint 022 wrote these to the Qdrant payload but `payload_to_item`
  never read them back, so no read path could project them. The
  knowledge→public mapping now lives in one place
  (`PublicMemoryContent::knowledge_from`), as does the author mapping
  (`PublicAuthorRef::from_record`); both were previously hand-rolled at
  four call sites, which is how the fields came to be dropped.
* **Incidental fix**: `matches_filters` reads `host` from the retrieval
  payload, but the vector payload never carried a `host` key — so a
  host-filtered knowledge query silently dropped every row. The payload
  now carries it.

## 2l. Sprint 026 deltas — retrieval measurement (#643)

The other half of sprint 026: make retrieval quality *measurable*, so the
ranking work in sprint 029 is a measured change rather than a guess.

* **The miss log was a dead instrument.** `LOW_SCORE_THRESHOLD` was 0.5,
  but bge-small cosine on this corpus sits ~0.75–0.96 **even for junk** (a
  content-free fragment measured 0.956) — the threshold sat below the
  floor of the distribution, so `low_score` could never fire. Two weeks of
  production recorded **one** miss, and that one was a `zero_hit` from an
  emptied filter. Recalibrated to **0.80**, inside the observed
  weak/strong boundary (~0.78–0.82, from #628's paired queries). This is a
  *calibrated* constant, honest only against the current embedding model —
  the #655 GPU model swap (sprint 028) changes the distribution wholesale
  and must re-derive it.
* **New: the search-sample log** (`search_sample`, migration 0011).
  Records **every** search — query, caller, top **raw** (pre-fusion) score,
  that hit's kind, hit count, kinds queried, and how many duplicates the
  #641 collapse removed. klams previously had no record of what agents ask
  it, which is why every eval query to date was invented rather than
  observed, and why the threshold above could only be calibrated from a
  handful of data points. Written fire-and-forget, exactly like
  `search_miss`; retention is an operator prune concern.
  - `top_raw_score` is deliberately the pre-fusion score: post-024 `score`
    is pure RRF and carries no magnitude, so a distribution over fused
    scores would say nothing about match quality.
* **Caller attribution fixed.** `record_search` was hardcoded to
  `"anonymous"` since sprint 020, so the per-agent search counter had
  exactly one label value — while the caller's agent name sat unused in
  the argument list. The miss log, the sample log, and the metric now
  share one `caller_label` helper so they cannot disagree.
* **`source_rank` is re-numbered after duplicate collapse.** Collapse
  removes entries, so survivors kept holed pre-collapse ranks (a caller
  asking for 20 could see ranks 0 and 25) — leaking the over-fetch
  multiplier as a gap. Ranks are now contiguous over the list the caller
  actually receives, restoring the sprint-017 invariant.
* **The eval suite is the gate** (`just eval`). The suite and runner live
  in klams-mind (`evals/suites/homelab-retrieval.toml`); the recipe lives
  here because klams is what regresses. It grew from 4 queries to 21, with
  three new check types — `no_duplicates` (#641's invariant),
  `min_body_chars` (the junk ceiling, breadcrumb stripped), and
  `memory_id` with `max_rank` (curated-beats-bulk; presence alone would
  have passed while #628 was live). Queries carry `expect`: `pass` is the
  regression bar, `known_open` is a tracked failure that does not fail the
  run. Without that distinction a measurement suite can only contain
  queries that already pass — which is exactly how the old four scored
  4/4 while every real failure was happening.

## 2m. Sprint 027 deltas — ingest correctness (#420 / #629 / #656)

### 2m.1 One ceiling, three enforcement points

`klams_types::EmbedLimit` is the single definition of "will the embedder
accept this text". It lives in `klams-types` because that is the only
crate the scanner, the API, and MCP all share — the same reason
`normalize_chunk_text` lives there.

```text
              [embeddings] max_input_tokens = 512
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
  scanner chunker     REST /index         MCP memory_add
  (char estimate:     (exact count        (exact count;
   cannot reach        before the 202,      this path had
   TEI; a 413 is       so the cursor        NO cap at all
   a counted skip,     never advances       before 027)
   never a stall)      past a lost chunk)
        └───────────────────┼───────────────────┘
                            ▼
                    TeiEmbedder preflight
                 (provable bound only — a
                  rejection here is final)
```

The REST and MCP gates ask the **model's own tokenizer** —
`Store::check_embed_size` → `Embedder::count_tokens` → TEI's
`POST /tokenize`, which runs no forward pass and so costs far less than
the failed embed it replaces.

That is not what this sprint set out to build. The plan called for a
conservative chars-per-token bound; measuring the live model showed no
such bound can work. Real content spans **1.03 chars/token**
(punctuation-dense) to **>39** (base64) — a 32× spread — so a divisor
safe for the former splits ordinary prose chunks in half, while one that
leaves prose alone under-counts the former threefold. The character
estimate (`klams_types::EmbedLimit`) therefore survives only where the
tokenizer is unreachable: the scanner, which talks solely to the klams
API. Its documentation carries the measurement table and states that it
is an approximation.

`tiktoken` is deliberately unused despite already being a workspace
dependency: `cl100k_base` is OpenAI's BPE vocabulary while bge-family
models use WordPiece, so it would be confidently wrong rather than
honestly approximate.

One subtlety worth preserving: the embedder's own preflight uses
`EmbedLimit::certainly_exceeds`, a *provable* bound (WordPiece never
merges across whitespace, so *n* words ⇒ ≥ *n* tokens) rather than the
estimate. A rejection there is final, and refusing on an estimate would
discard token-efficient content the model accepts — a new source of lost
writes in the sprint meant to end them.

### 2m.2 The silent-loss triangle, closed

Three independent gaps combined into invisible data loss (review F-3.2):

1. REST accepted ≤8192 characters, ~4× the model's real capacity.
2. The worker embeds with **no reply channel** for knowledge; on failure
   it logged and dropped the job without incrementing `writes_failed`,
   because that counter was only ever touched by HTTP handlers.
3. The scanner had already advanced its cursor on the `202`, so nothing
   ever retried.

All three are now closed: the size gate makes (1) impossible, the worker
increments `writes_failed{type,reason}` and logs the loss explicitly at
(2), and rejecting before the `202` means (3) cannot arise.

`/healthz` is deliberately unchanged. TEI's `/health` returns 200
whenever the model is loaded — input rejections never touch it — so
health was never going to catch this. The dropped-write counter is the
right instrument, not a health check.

### 2m.3 The transient/permanent axis

`StoreError` gained a classification (`Transience`) because the
information needed to answer "should the caller retry?" was being
destroyed at the HTTP boundary and could not be recovered downstream.
The embedder retry loops retried *every* non-2xx three times and
discarded the response body — including TEI's `inputs must have less
than 512 tokens`, the one actionable string it sends.

Now: 4xx fails immediately with the body captured, 5xx/connect/timeout
retry, and Postgres failures are classified by SQLSTATE
(`08`/`40`/`53`/`57` → transient) via `StoreError::from_sqlx`. The MCP
layer maps all of it through one function, `errors::from_store_error`,
which enforces the invariant that makes the contract usable:
`retry_after_seconds` is present **iff** the error is transient.

### 2m.4 The oversize-write log

`oversize_write` (migration 0012) records refused knowledge writes
*including the full submitted text*. It mirrors the `search_miss`
pattern — fire-and-forget, never affecting what the caller sees — with
one deliberate difference: it is **pruned on a daily timer** rather than
left to the operator, because it retains whole documents.

Its purpose is evidential. Sprint 028 raises the ceiling to 8k+ tokens,
which should make oversize rejections rare; this table is what decides
whether #632's server-side chunking is ever worth building, rather than
building it on the assumption that it is.

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
[research.md §3](../sprints/001-initial-mvp/research.md#3-klams-service-deployment).

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
  [research.md §7](../sprints/001-initial-mvp/research.md#7-auth-model-for-mvp).

## 4. Where to look next

* End-to-end provisioning steps: [setup.md](setup.md).
* Day-to-day operator recipes (start/stop, log inspection, viewport
  install): [usage.md](usage.md).
* MVP smoke checks mapped to success criteria:
  [sprints/001-initial-mvp/quickstart.md §9](../sprints/001-initial-mvp/quickstart.md#9-smoke-test-the-user-stories).
* Per-decision rationale:
  [sprints/001-initial-mvp/research.md](../sprints/001-initial-mvp/research.md).
