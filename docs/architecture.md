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
│   └── klams.service                       runs the klams-service binary
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
