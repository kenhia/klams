# Phase 0 Research: klams Initial MVP

This file resolves the open items the spec deferred to plan phase, and
records the rationale for each technology decision. Each entry uses:

- **Decision** — what was chosen
- **Rationale** — why
- **Alternatives considered** — what else was evaluated and why rejected

## 1. Embedding model + runtime

**Decision**: Run Hugging Face
[Text Embeddings Inference (TEI)](https://github.com/huggingface/text-embeddings-inference)
as a Docker container (`ghcr.io/huggingface/text-embeddings-inference:1.5-gpu`
or current stable) with `--model-id BAAI/bge-small-en-v1.5`, GPU access
via `--gpus all`, listening on an internal Compose network. The klams
service calls TEI's `POST /embed` over HTTP (`reqwest`).

**Rationale**:
- TEI is purpose-built for embedding workloads, has first-class CUDA
  support, batches on the server side, exposes Prometheus metrics, and
  handles VRAM lifecycle (releases on idle), matching the
  "GPU shared with other workloads" constraint.
- `bge-small-en-v1.5` is 384-dim, ~120 MB on disk, ~400 MB VRAM at
  inference, top-10 on MTEB for its size class. Cheap enough to embed
  the entire Obsidian vault in seconds while leaving most of the GPU
  for other agents.
- Calling embedding over HTTP isolates model concerns from the Rust
  service: model upgrades, batching tuning, and CPU/GPU fallback are
  configured at the TEI container without rebuilding klams.
- Plays cleanly with the "no embedded vector store, services run in
  containers" directive.

**Alternatives considered**:
- **`candle` in-process embedding**: keeps everything in one binary
  but bakes the model choice into the Rust build, complicates GPU
  lifecycle, and makes upgrading models painful. Rejected for MVP.
- **`nomic-embed-text` via Ollama**: works, but Ollama optimizes for
  chat completion latency, not embedding throughput, and brings a
  bigger dependency. Reject — TEI is the right tool.
- **`text-embedding-3-small` via OpenAI**: violates "everything stays
  in the homelab" non-goal from plan.md §1. Rejected.

**Open knobs for the implementation phase**:
- TEI batch size and concurrency flags — tuned during Phase 2 perf work.
- Vector dimension is fixed at 384 from this choice; baked into the
  Qdrant collection config in `klams-store`.

## 2. PostgreSQL deployment mode

**Decision**: Dedicated PostgreSQL 16 container in
`deploy/docker-compose.yml`, named `klams-postgres`, with a dedicated
database `klams` and role `klams`. Bind volume
`${KLAMS_DATA_ROOT}/postgres:/var/lib/postgresql/data` (default
`KLAMS_DATA_ROOT=/ai/klams/data`; see §12). Bound to `127.0.0.1:5432`
on the host so it is reachable from systemd-managed processes without
exposing on the LAN. Attached to the shared `klams-net` user-defined
bridge network as service alias `postgres` (see §13).

**Rationale**:
- Isolation: klams owns its DB outright, simplifying backups and
  migration timing.
- Versioning: independent of any other Postgres on `kubs0`.
- Compose makes provisioning a one-liner and matches the user's
  "services via docker containers" directive.
- Bind-mounted data dir on the host filesystem keeps backups simple
  (`pg_dump` reachable from the host) and survives container recreation.
- Single rooted data tree under `/ai/klams` makes backups, NAS
  snapshots, and capacity planning trivial.

**Alternatives considered**:
- **Shared kubs0 Postgres instance**: less ops surface today, but
  every backup, version upgrade, or downtime decision becomes
  cross-project. Rejected.
- **Named volume instead of bind mount**: hides data inside Docker's
  storage tree, complicates backups. Rejected for the MVP.

## 3. klams service deployment

**Decision**: Run `klams-service` as a **systemd unit** on `kubs0`
(`deploy/systemd/klams.service`), with PostgreSQL, Qdrant, and TEI
running as Compose services. Provide a commented-out `klams` service
block in `docker-compose.yml` for developers who prefer Compose during
local iteration.

**Rationale**:
- The service is a Rust binary with minimal runtime needs and no
  external state of its own. Running it under systemd gives:
  - Direct journald integration for `tracing` output without an extra
    log driver.
  - `Restart=on-failure` semantics without Compose restart loops.
  - Easier to attach `gdb`, profile, and update via `cargo install`.
- Keeping stateful services in Compose and the stateless service under
  systemd is a clean separation of concerns appropriate for a
  single-host homelab.
- Compose-for-dev path remains available for contributors who do not
  want to install the systemd unit.

**Alternatives considered**:
- **Everything in Compose, including klams**: simpler "one button"
  startup, but worse log access and slower dev loop (rebuild image to
  test changes). Rejected for MVP; revisit if multi-host deployment
  becomes a goal.
- **Bare process under tmux**: not durable across boot. Rejected.

## 4. Qdrant deployment

**Decision**: Qdrant 1.x as a Compose service `klams-qdrant`, on-disk
persistence with bind mount
`${KLAMS_DATA_ROOT}/qdrant:/qdrant/storage` (default
`/ai/klams/data/qdrant`; see §12), bound to `127.0.0.1:6333` (HTTP)
and `127.0.0.1:6334` (gRPC). klams uses the **gRPC** client.
Single-node, no replication. Attached to the shared `klams-net`
network as service alias `qdrant` (see §13).

**Rationale**:
- On-disk persistence is mandatory for SC-004 (survive restart).
- gRPC is the recommended high-throughput client for Qdrant and is
  well supported by the official `qdrant-client` Rust crate.
- LAN binding (127.0.0.1) keeps Qdrant invisible outside the host;
  klams (systemd) reaches it via loopback.

**Alternatives considered**:
- **In-memory Qdrant**: violates SC-004. Rejected.
- **Embedded vector store (e.g., LanceDB in-process)**: contradicts
  the user's "Qdrant should not be embedded if/when used" directive.
  Rejected.

## 5. HTTP framework + Prometheus

**Decision**: `axum` 0.7+ on `tokio`, with `tower-http` middleware for
tracing and request IDs, and `axum-prometheus` for `/metrics`.

**Rationale**:
- Mainstream Rust async HTTP stack with first-class `tower` integration
  for auth and rate-limit middleware.
- `axum-prometheus` exposes the histograms required by FR-018 without
  bespoke code.
- Native handler ergonomics match `serde`-typed DTOs in `klams-types`.

**Alternatives considered**:
- **`actix-web`**: equally capable but a separate runtime story, and
  the team is more familiar with tower-based stacks. Rejected on
  familiarity grounds.
- **`hyper` directly**: too low-level for the MVP timeline.

## 6. Viewport stack + cross-build

**Decision**: Tauri 2.x backend (Rust) + Svelte 5 frontend via
SvelteKit (static adapter) bundled by Vite. Cross-built from Linux to
`x86_64-pc-windows-msvc` using
[`cargo-xwin`](https://github.com/rust-cross/cargo-xwin) for the Tauri
backend, with Vite producing the frontend SPA bundle in
`viewport/build/` (or whatever `svelte.config.js` static adapter
outputs).

Build invocation:

```bash
# from viewport/
pnpm install
pnpm build                 # SvelteKit static SPA into ./build
cd src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
# output:
# src-tauri/target/x86_64-pc-windows-msvc/release/klams-viewport.exe
```

No MSI installer. The single `.exe` is what gets copied to the Windows
workstation.

**Rationale**:
- Per spec FR-022 and the user's clarifying answer, only a binary is
  required; skipping MSI bundling avoids needing `wixtools` and a
  Windows runner.
- `cargo-xwin` is the de-facto Linux-to-Windows cross-build path for
  Rust; downloads the Microsoft xwin SDK once and caches it locally
  (gitignored at `viewport/xwin/`).
- SvelteKit's static adapter produces a fully prerendered SPA that
  Tauri can serve from its embedded asset server (`distDir` in
  `tauri.conf.json`) — no Node runtime in the shipped binary.

**Alternatives considered**:
- **Build on the Windows workstation directly with `pnpm tauri build`**:
  works, but requires keeping a Rust + Node toolchain in sync on
  Windows. Documented as a fallback in `viewport/README.md`, not the
  default path.
- **Plain Vite + Svelte (no Kit)**: simpler routing story, but
  SvelteKit's static adapter handles the multi-route MVP UI
  (`/facts`, `/events`, `/knowledge`) more cleanly. Kept Kit.
- **Tauri 1.x**: nearing EOL; Tauri 2 is the current LTS line.
  Rejected.

**Note on the referenced example**:
The user mentioned `~/src/tools/kpidash/clients/kpidash-client` as a
Tauri/Svelte cross-build example. That directory is a Python client.
No Tauri example was found under `~/src/tools/kpidash` at planning
time. We proceeded with the well-documented standard `cargo-xwin`
recipe above. If a different reference repo exists, plan can be
updated during Phase 2 task work without affecting the contract.

## 7. Auth model for MVP

**Decision**: A single bearer token loaded from
`/etc/klams/klams.toml` (or a path overridable via
`KLAMS_CONFIG`). All `/memory/*` endpoints require it. `/healthz` and
`/metrics` are unauthenticated (standard practice) but bound to a
LAN-only interface via the systemd unit's `BindIP=` or by configuring
the listen address to a private interface.

Comparison is constant-time (`subtle::ConstantTimeEq` or equivalent).
Token is treated as a secret in logs (redacted via `tracing`'s
field filtering).

**Rationale**:
- Matches spec FR-016 and plan.md §5: "local-network only, bearer
  token from a config file".
- Defers per-client tokens, rotation, and TLS to later phases per the
  user's deferral list.

**Alternatives considered**:
- **mTLS**: overkill for a homelab LAN MVP; deferred.
- **No auth (LAN trust)**: cheaper still, but exposes the API to any
  process on `kubs0`, including curious agents. Rejected.

## 8. Logging and error mapping

**Decision**:
- `tracing` + `tracing-subscriber` with the `json` formatter when
  `KLAMS_LOG_FORMAT=json` is set (production / journald); pretty
  formatter for dev.
- Error type per crate (`thiserror`), mapped to HTTP responses by an
  `IntoResponse` impl on the API crate's top-level `ApiError`.
- 4xx errors carry `{ code, message, field? }` JSON bodies; 5xx
  errors carry `{ code, message }` only and the full error is logged
  at ERROR level with the request id.

**Rationale**: Satisfies constitution principle V "actionable errors"
and FR-019 "errors include sufficient context without exposing the
bearer token".

**Alternatives considered**: `slog` (older, fewer integrations),
`log` + `env_logger` (no spans). Both rejected.

## 9. Dedupe strategy

**Decision**:
- **Facts**: canonical-JSON serialization of `payload` (sorted keys,
  no whitespace) + `type` → SHA-256 → store as `payload_hash BYTEA`
  with a unique index `(type, payload_hash)`. Upsert via
  `INSERT … ON CONFLICT (type, payload_hash) DO UPDATE`.
- **Knowledge**: SHA-256 of normalized `text` (NFC + trim + collapse
  whitespace) → store in Qdrant payload as `content_hash`. Before
  enqueueing, query Qdrant for an existing point with the same
  `content_hash` (filter, no vector); if found, return that
  `knowledge_id` and skip embedding.

**Rationale**: Cheap, deterministic, and protects storage from
duplicate work; sufficient for the MVP per spec FR-014.

**Alternatives considered**:
- **MinHash / SimHash near-duplicate detection**: useful for messy
  scrapes, but plan.md defers semantic dedupe to later phases.
  Rejected.

## 10. Migrations

**Decision**: `sqlx` migrate, with SQL files under `migrations/`.
Migrations are applied automatically at service startup (idempotent),
and also runnable via `sqlx migrate run` in CI for the test compose
stack.

**Rationale**: Single-developer MVP; auto-migrate keeps the dev loop
fast. The Phase 5 backup plan will move auto-migrate behind a feature
flag, but that is out of scope here.

## 11. Test strategy summary

- **Unit**: per-crate `#[test]` and `#[tokio::test]`; mock Postgres /
  Qdrant via traits where convenient.
- **Integration**: a `tests/` directory inside `crates/klams-service`
  spawns the binary against a Compose stack defined in
  `tests/docker-compose.test.yml` (ephemeral volumes; teardown after
  run). CI starts the stack via `docker compose up -d` and runs
  `cargo test --workspace`.
- **Viewport**: minimal `vitest` for `lib/api.ts` wrappers; manual
  smoke test on Windows for the MVP. End-to-end UI testing deferred.

## 12. Storage root

**Decision**: All klams persistent state — Compose service data
volumes (Postgres, Qdrant, TEI model cache), the klams service's own
state directory (config snapshots, sqlx offline data, logs that are
not in journald), and any future Compose-attached services (Redis,
Grafana, etc.) — live under a single configurable root, default
`/ai/klams`.

Layout under the root:

```text
/ai/klams/
├── config/                # bearer token, service config, env file
│   ├── klams.toml
│   └── compose.env        # KLAMS_DATA_ROOT, image tags, ports
├── data/                  # all stateful service volumes
│   ├── postgres/
│   ├── qdrant/
│   └── tei/               # HF model cache
└── logs/                  # optional non-journald logs
```

- The root is exposed to Compose as `KLAMS_DATA_ROOT` (default
  `/ai/klams/data`) and to the klams binary as `KLAMS_ROOT` (default
  `/ai/klams`) — the latter being the top-level path under which
  `config/`, `data/`, and `logs/` all live.
- `deploy/compose.env` (gitignored) is loaded by Compose via
  `--env-file` and contains `KLAMS_DATA_ROOT=/ai/klams/data` and
  the bearer token reference.
- `/etc/klams/klams.toml` is symlinked to `/ai/klams/config/klams.toml`
  by the deploy step; the systemd unit reads `KLAMS_CONFIG` from its
  EnvironmentFile.
- The generated config files are created by `/speckit.tasks` Phase 0
  setup tasks, not committed to the repo. The repo ships
  `deploy/config/klams.example.toml` and `deploy/compose.env.example`
  with the documented defaults; the install task copies and edits.

**Rationale**:
- Centralizes all state under one path → easy NAS snapshot, easy `du`,
  easy `rsync` migration to another host.
- Configurable so dev boxes (e.g., `kai`) can use a different root
  without code changes.
- Keeps secrets and state outside the git working tree.

**Alternatives considered**:
- **Default Docker named volumes**: opaque, harder to back up, no
  single path to snapshot. Rejected.
- **Per-service roots scattered under `/var/lib/`**: violates the
  single-tree backup goal. Rejected.

## 13. Docker network

**Decision**: All Compose services in `deploy/docker-compose.yml`
(and in `tests/docker-compose.test.yml`) attach to a single
**user-defined bridge network** named `klams-net`. Each service has
a deterministic `name`/alias so peers can resolve it via DNS at the
bare name (`postgres`, `qdrant`, `tei`, and future `redis`, `grafana`,
etc.).

Compose snippet pattern:

```yaml
networks:
  klams-net:
    name: klams-net
    driver: bridge

services:
  postgres:
    image: postgres:16
    networks:
      klams-net:
        aliases: [postgres]
    ports: ["127.0.0.1:5432:5432"]
    volumes:
      - ${KLAMS_DATA_ROOT}/postgres:/var/lib/postgresql/data

  qdrant:
    image: qdrant/qdrant:v1.12.4
    networks:
      klams-net:
        aliases: [qdrant]
    ports:
      - "127.0.0.1:6333:6333"
      - "127.0.0.1:6334:6334"
    volumes:
      - ${KLAMS_DATA_ROOT}/qdrant:/qdrant/storage

  tei:
    image: ghcr.io/huggingface/text-embeddings-inference:1.5
    networks:
      klams-net:
        aliases: [tei]
    ports: ["127.0.0.1:7070:80"]
    volumes:
      - ${KLAMS_DATA_ROOT}/tei:/data
```

The test compose file (`tests/docker-compose.test.yml`) defines its
own isolated network `klams-test-net` following the same pattern,
with different host port bindings to avoid colliding with the
production stack.

**Service-to-service URLs** (inside the network):

| Caller | Target | URL |
|---|---|---|
| klams (Compose-mode, if enabled) | Postgres | `postgres://klams:…@postgres:5432/klams` |
| klams (Compose-mode) | Qdrant gRPC | `http://qdrant:6334` |
| klams (Compose-mode) | TEI | `http://tei:80` |
| klams (systemd on host) | Postgres | `postgres://klams:…@127.0.0.1:5432/klams` |
| klams (systemd on host) | Qdrant gRPC | `http://127.0.0.1:6334` |
| klams (systemd on host) | TEI | `http://127.0.0.1:7070` |

The klams config file (`klams.toml`) holds the URLs as plain strings
so a single config can be flipped between Compose-internal DNS names
and host loopback without code changes. The example config ships
with the host-loopback variant (matching the MVP systemd deployment).

**Rationale**:
- DNS-based discovery (`postgres:5432`) is deterministic,
  reproducible, and survives container restarts/recreations.
- A named network keeps klams services discoverable across
  independently-managed Compose projects (e.g., a future Grafana
  Compose file can `external: true` into `klams-net`).
- The default bridge network is the Docker antipattern for any
  multi-service deployment; explicitly forbidding it removes a class
  of "works on my machine" bugs.

**Alternatives considered**:
- **Default `bridge` network**: no DNS for service names, requires
  container IPs or `links:`. Rejected outright per the user's
  networking requirement.
- **`host` network mode**: simpler but defeats container isolation
  and makes port conflicts loud. Rejected.
- **One network per service pair**: more isolation, much more YAML
  for no MVP benefit. Rejected.

## 14. Deferred (out of scope here)

The following are explicitly out of scope and will be picked up in
later spec iterations per plan.md:

- Decay scoring, hallucination filters, schema validator framework
  (Phase 2)
- Non-agentic writers — Ansible callback, repo scanner, service
  monitors (Phase 3)
- `/memory/context` retrieval bundle + summarization (Phase 4)
- Backups, Grafana dashboards, restore drills, `maintenance_mode`
  (Phase 5)
- MCP server for GHCP, per-agent quotas, projection layer (Phase 6)
- TLS, mTLS, per-client tokens, secret rotation
- Viewport write/override UI, context preview, agent activity panel
- Viewport MSI installer, code signing, auto-update
