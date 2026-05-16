# klams — Ken's Local Agent Memory System

**Status:** Planning  
**Target host:** `kubs0` (production + future development)  
**Current host:** `kai` (initial planning only — repo will move to `kubs0`)  
**Owner:** Ken  

## 1. Vision

`klams` is a controller-centric, shared memory service for Ken's homelab agent
ecosystem. It gives the controller, GitHub Copilot (GHCP), and future agents a
unified, durable, and observable place to read and write three kinds of memory:

- **User memory** — stable facts about Ken, his machines, preferences,
  and homelab topology.
- **Task memory** — repos, services, sprint state, execution traces, and
  events.
- **Knowledge memory** — semantic content from the Obsidian vault, specs,
  READMEs, and troubleshooting notes.

The service is async, GPU-aware (for embeddings), runs on `kubs0` alongside its
data stores, and exposes a stable API that agents — and a desktop GUI — can
plug into.

### Non-goals (initial scope)

- Multi-tenant isolation
- Horizontal scaling across nodes
- Strong consistency across multiple memory instances
- Cloud-hosted components (everything stays in the homelab; backups may sync
  off-site)

## 2. Source material

This plan supersedes and operationalizes the earlier exploratory notes:

- `~/obsidian/gratch/Computers and Services/MemoryServer/Spec.md`
- `~/obsidian/gratch/Computers and Services/MemoryServer/Phases.md`

Those notes captured architecture and a phased roadmap. This document refines
them into actionable iterations consistent with the klams
[constitution](../../.specify/memory/constitution.md), and adds the
[memory viewport desktop GUI](viewport.md) as a first-class deliverable from
day one.

## 3. Architecture summary

### 3.1 Components (on `kubs0`)

| Component | Tech | Role |
|---|---|---|
| `memory-service` | Rust binary (tokio) | API, queue, worker pool, embedding pipeline |
| Postgres | Existing kubs0 instance or dedicated DB | `facts`, `events` |
| Qdrant | Native install on kubs0 | `knowledge_items` vector collection |
| Embedding model | GPU-accelerated on kubs0 | Async embedding generation |
| Prometheus exporter | Built into service | Metrics scraping |

### 3.2 Clients

- **Controller** (on other machines) — HTTP/gRPC client
- **GHCP** — via MCP server (Phase 6)
- **Non-agent tasks** — Ansible plays, repo scanners, service monitors
- **Memory viewport** — Tauri/Svelte desktop app on Windows; see
  [viewport.md](viewport.md)

### 3.3 Pipeline

```
client → API → enqueue MemoryWrite → bounded mpsc queue
                                       ↓
                       homogeneous worker pool (N workers)
                                       ↓
                      validate → dedupe → policy → write
                                       ↓
                          Postgres / Qdrant + metrics
```

Reads bypass the queue and hit Postgres/Qdrant directly with decay-aware
scoring.

### 3.4 Workspace layout (Rust)

```
crates/
  memory-types/        # shared structs, enums, MemoryWrite
  memory-core/         # queue, workers, policy, decay
  memory-store/        # Postgres + Qdrant adapters
  memory-api/          # HTTP/gRPC server
  memory-service/      # binary (wires everything)
  memory-mcp/          # MCP server (Phase 6)
viewport/              # Tauri + Svelte app (see viewport.md)
docs/
  architecture.md
  setup.md
  usage.md
specs/
```

## 4. Data model (MVP)

### 4.1 Fact (Postgres `facts`)

```
id UUID PK
type TEXT NOT NULL          -- UserFact | TaskFact | EnvFact | ...
payload JSONB NOT NULL      -- typed by `type`
version INT NOT NULL
source TEXT NOT NULL        -- User | Controller | Task | AgentProposal
created_at TIMESTAMPTZ
updated_at TIMESTAMPTZ
last_used_at TIMESTAMPTZ
use_count INT
confidence REAL
decay_weight REAL
```

Indexes: `(type)`, `(source)`, `(created_at)`, GIN on `payload`.

### 4.2 Event (Postgres `events`, append-only)

```
id UUID PK
task_id UUID NULL
category TEXT NOT NULL      -- Execution | Service | Repo | ...
payload JSONB NOT NULL
source TEXT NOT NULL
created_at TIMESTAMPTZ
```

### 4.3 Knowledge item (Qdrant `knowledge_items`)

- vector: embedding
- payload: `text`, `source`, `tags`, `repo`, `file`, `machine`,
  `created_at`, `updated_at`, `last_used_at`, `use_count`,
  `confidence`, `decay_weight`

### 4.4 `MemoryWrite` enum (core pipeline type)

```rust
enum MemoryWrite {
    UserFactUpsert(UserFactWrite),
    TaskFactUpsert(TaskFactWrite),
    EventAppend(EventWrite),
    KnowledgeIndex(KnowledgeWrite),
    KnowledgeUpdate(KnowledgeUpdate),
}
```

## 5. APIs (MVP)

- `POST /memory/facts` — upsert User/Task facts
- `POST /memory/events` — append events
- `POST /memory/knowledge/index` — index a document chunk
- `POST /memory/search` — unified search (`types`, `filters`, `top_k`)
- `GET /memory/facts` — list/browse facts (debugging + viewport)
- `GET /healthz`, `GET /metrics`

Auth for MVP: local-network only, bearer token from a config file. Future:
per-client tokens.

## 6. Write policy and validation

| Source | Trust | Behavior |
|---|---|---|
| User | Highest | Direct write; wins conflicts |
| Controller | Trusted | Direct write |
| Task / Ansible / scanners | Trusted | Direct write |
| Agent | Untrusted | Validated proposal; cannot override user-set facts without explicit flag |

- Schema validation per `type`
- Optimistic concurrency on `facts.version`
- Conflict rules: User memory → user wins; Task memory → newest wins;
  Knowledge memory → merge metadata, replace embedding/text

## 7. Retrieval and decay

Score:

$$
\text{score} = \text{relevance} \times \frac{1}{1 + \lambda \cdot \text{age}} \times \log(1 + \text{use\_count}) \times \text{confidence}
$$

Per-type `λ`:

- Machine facts: ≈ 0 (no decay)
- Preferences/interests: slow
- Task state: faster
- Working/ephemeral: very fast

A background task updates `decay_weight` and `last_used_at` periodically.

## 8. Phased roadmap

Each phase produces a shippable increment. Phase exit criteria are
binary — pass or not. Every phase ends with the
[constitution's pre-commit gates](../../.specify/memory/constitution.md#pre-commit-checks)
and updates to `docs/`.

### Phase 0 — Foundations

**Goal:** Skeleton service + viewport scaffold + working dev environment on
`kubs0`.

Deliverables:

1. Repo moved from `kai` to `kubs0`; CI runs on `kubs0` or GitHub-hosted
   runner.
2. Rust workspace + crates listed in §3.4 with empty `lib.rs` stubs.
3. `MemoryWrite` enum defined in `memory-types`.
4. Bounded `tokio::mpsc` queue + homogeneous worker pool in `memory-core`
   that logs job receipt.
5. Stub HTTP API in `memory-api` accepting requests and enqueuing them.
6. Stub Postgres + Qdrant clients in `memory-store` (connect on startup,
   no schema yet).
7. Structured logging via `tracing`; Prometheus exporter at `/metrics`
   with queue depth + worker count.
8. Postgres database `klams` provisioned on `kubs0` (or shared instance,
   TBD during planning of this phase).
9. Qdrant installed and running on `kubs0`.
10. Viewport scaffold (`viewport/`) per [viewport.md §3](viewport.md#3-phase-0-scaffold).
11. `docs/architecture.md` and `docs/setup.md` created.

Exit criteria: service starts on `kubs0`, accepts a stub `POST /memory/facts`
that round-trips through the queue and logs, viewport builds and launches on
Windows showing a placeholder window.

### Phase 1 — MVP memory

**Goal:** End-to-end working memory; controller can read/write.

Deliverables:

1. Postgres migrations: `facts`, `events` tables with indexes.
2. Qdrant collection `knowledge_items` created via service bootstrap.
3. Implement `MemoryWrite` handlers in workers (validate → dedupe → write).
4. Implement reads: `GET /memory/facts`, `POST /memory/search` (structured
   + vector with simple weighted hybrid score).
5. Embedding pipeline: pick a model (decision deferred to phase planning;
   candidates: `bge-small-en-v1.5`, `nomic-embed-text`), run on GPU,
   async batch insert.
6. Dedupe: hash-based for facts (type + canonical payload), content-hash
   for knowledge items.
7. Controller integration: minimal Rust client crate `klams-client`
   under `crates/`.
8. Viewport: klams memory inspector (read-only) working end-to-end on
   Windows against the Phase 1 service; see
   [viewport.md §4](viewport.md#4-phase-1-klams-memory-inspector).
9. `docs/usage.md` covers controller integration.

Exit criteria: controller and a manual `curl` workflow can write a fact,
index a knowledge chunk, and find both via `/memory/search`. Embeddings
persist across service restarts.

### Phase 2 — Safety, drift control, and the user view

**Goal:** Reliable, self-correcting memory; humans can inspect and override.

Deliverables:

1. Schema validation layer per `type` (using `serde` + per-type validator
   functions).
2. Conflict resolution per §6, with explicit `override_user_fact` flag.
3. Decay model + background `tokio` task updating `decay_weight`.
4. Hallucination filters for agent-sourced writes (basic: required-field
   checks, value-range checks).
5. Viewport: write operations (delete / override) and provenance panel
   added to the memory inspector; see
   [viewport.md §5](viewport.md#5-phase-2-write-operations-and-provenance).

Exit criteria: a malformed agent write is rejected with an actionable
error; user-set facts survive agent proposals; viewport can delete a
fact and the change is visible on next read.

### Phase 3 — Non-agentic writes and integrations

**Goal:** The system updates itself.

Deliverables:

1. Ansible callback or post-play hook publishes facts to `/memory/facts`.
2. Repo scanner (cron or systemd timer) walks `~/src` and `~/obsidian`,
   chunks, embeds, and indexes into knowledge memory.
3. Service monitors push task memory events (service up/down, version,
   port).
4. Controller execution traces flow into `events`.
5. Per-source write policy enforced (trusted sources skip the proposal
   path).

Exit criteria: provisioning a new machine via Ansible automatically
populates its user-memory facts in klams; a new note in the Obsidian vault
is searchable in klams within one scan cycle.

### Phase 4 — Advanced retrieval and summarization

**Goal:** Less noise, better context.

Deliverables:

1. Hybrid retrieval: vector + keyword (Postgres FTS) + metadata filters.
2. Temporal weighting tuning per memory type (config-driven).
3. Summarization pipelines (background task) for long event logs and
   stale knowledge clusters.
4. `POST /memory/context` — "retrieve for agent" endpoint that returns
   structured facts + relevant knowledge + recent events, deduped and
   summarized, with a token-budget parameter.
5. Viewport: search UI that previews `/memory/context` output for a
   query; see [viewport.md §6](viewport.md#6-phase-4-context-preview).

Exit criteria: `/memory/context` returns a coherent context bundle under
a configurable token budget for a representative query.

### Phase 5 — Maintenance, backups, and ops

**Goal:** Production-stable in the homelab.

Deliverables:

1. Nightly `pg_dump` of klams DB to `gratch` NAS.
2. Qdrant snapshot/sync to `gratch` during a maintenance window.
3. Optional cloud sync of `gratch` artifacts (out of scope for klams;
   relies on existing gratch backup chain).
4. `maintenance_mode` flag — service rejects non-critical writes,
   allows reads.
5. Grafana dashboards: queue health, worker throughput, error rates,
   latency distributions, backup status.
6. Restore procedure documented in `docs/setup.md` and tested at least
   once.

Exit criteria: backup runs nightly without intervention; restore test
brings up a fresh klams from yesterday's snapshot.

### Phase 6 — MCP memory server (external agents)

**Goal:** GHCP and other agents can use klams safely.

Deliverables:

1. `memory-mcp` crate exposing MCP tools:
   `memory.add`, `memory.search`, `memory.related`, `memory.delete`.
2. Projection layer: internal schema → simplified public schema (hides
   event logs, raw embeddings, decay metadata, controller-only fields).
3. Per-agent rate limits and quotas.
4. MCP server registered in `~/.copilot` config and tested with GHCP.
5. Viewport: agent activity panel — recent agent proposals, accepted /
   rejected.

Exit criteria: GHCP can add a memory item via MCP, query it back, and
the proposal appears in the viewport's agent activity panel.

### Phase 7 — Optional enhancements (backlog)

Items here move to [backlog.md](backlog.md) and graduate to a phase
when prioritized:

- Multi-vector embeddings (text + code)
- Lightweight graph memory
- Memory diffing and replay
- Cross-machine caching
- Multi-agent coordination memory

## 9. Risks and open questions

| Risk / question | Mitigation / decision needed at |
|---|---|
| Embedding model choice (GPU memory, throughput) | Phase 1 planning |
| Postgres: dedicated DB vs shared kubs0 instance | Phase 0 planning |
| Qdrant version + on-disk vs in-memory mode | Phase 0 planning |
| HTTP vs gRPC for internal API | Phase 0 planning (default: HTTP/JSON for MVP) |
| Auth model beyond bearer token | Phase 6 planning |
| Viewport binary distribution (signed installer for Windows) | Phase 1 planning |
| Token-budget heuristics for `/memory/context` | Phase 4 planning |

## 10. Definition of done (per phase)

For every phase:

1. All deliverables listed above are merged.
2. Pre-commit checks pass on CI:
   `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.
3. `docs/architecture.md`, `docs/setup.md`, `docs/usage.md` reflect the
   shipped state.
4. A spec under `specs/NNN-<phase>/` exists with `spec.md`, `plan.md`,
   and `tasks.md`.
5. The viewport (when impacted by the phase) is updated and a build
   artifact is produced for Windows.

## 11. Next steps

1. Move repo from `kai` to `kubs0` and confirm dev environment.
2. Open `specs/001-phase-0-foundations/` and run the `/speckit.specify`
   workflow to break Phase 0 into concrete tasks.
3. In parallel, scaffold the [viewport](viewport.md) — it's the
   long-pole debugging tool, so it should start early.
