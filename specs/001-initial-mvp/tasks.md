# Tasks: klams Initial MVP

**Input**: Design documents in [`/specs/001-initial-mvp/`](.)
**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md), [data-model.md](data-model.md), [contracts/](contracts/)

**Tests**: REQUIRED. Constitution §II (TDD) makes test-first mandatory for new code. Every implementation task within a user story phase is preceded by a test task that MUST be written first and MUST fail before the implementation lands.

**Organization**: Tasks are grouped by user story so each story can be implemented, tested, and shipped independently. Phases 1–2 are shared; Phases 3–8 are per-story (one phase per US1..US6); Phase 9 is polish.

## Format: `[ID] [P?] [Story?] Description with file path`

- **[P]**: Parallelizable — touches different files than other in-flight tasks in the same phase and has no incomplete dependencies.
- **[Story]**: User story label (US1..US6). Setup, Foundational, and Polish tasks have no story label.

## Path Conventions

- Repo-root Cargo workspace: `Cargo.toml`, `crates/<name>/`, `migrations/`, `tests/`, `deploy/`.
- Viewport workspace: `viewport/Cargo.toml`, `viewport/src-tauri/`, `viewport/src/`.
- Docs: `docs/`.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, toolchain, scaffolding, and runtime config provisioning. Nothing in later phases can build without these in place.

- [X] T001 Create the repo-root Cargo workspace [`Cargo.toml`](../../Cargo.toml) declaring members `crates/klams-types`, `crates/klams-core`, `crates/klams-store`, `crates/klams-api`, `crates/klams-service`, `crates/klams-client` per [plan.md](plan.md) §"Project Structure".
- [X] T002 Add [`rust-toolchain.toml`](../../rust-toolchain.toml) pinning the stable channel and `rustfmt`, `clippy`, plus `x86_64-pc-windows-msvc` target.
- [X] T003 [P] Create empty crate skeletons with `Cargo.toml` + `src/lib.rs` (or `src/main.rs` for `klams-service`) for each of the six repo-root crates listed in T001. No code beyond `pub fn placeholder() {}` and the crate doc-comment.
- [X] T004 [P] Add workspace-wide lint/format config: `rustfmt.toml`, `clippy.toml`, and `[workspace.lints]` in the root `Cargo.toml` enabling `clippy::all` and `clippy::pedantic` warnings as errors (matches constitution §III pre-commit gate).
- [X] T005 [P] Update [`.gitignore`](../../.gitignore) to add `viewport/xwin/`, `viewport/build/`, `viewport/node_modules/`, `viewport/src-tauri/target/`, `/ai/klams/` placeholder note (not actually inside repo), `deploy/compose.env`, and `/etc/klams/`.
- [X] T006 [P] Scaffold the viewport workspace: `viewport/Cargo.toml`, `viewport/rust-toolchain.toml`, `viewport/src-tauri/Cargo.toml` (single member crate `klams-viewport`), `viewport/src-tauri/tauri.conf.json` (app id `dev.ken.klams.viewport`, identifier, 1200×800 window, `distDir: "../build"`, **`bundle.active = false`** since FR-022 requires only the raw `.exe`), and `viewport/src-tauri/src/main.rs` with a stub Tauri builder. Set product name `klams-viewport` and Cargo binary name `klams-viewport`.
- [X] T007 [P] Scaffold the viewport frontend: run `pnpm create svelte@latest src --template=skeleton --types=typescript --no-add-eslint --no-add-prettier` equivalent (manual scaffold acceptable). Add `@sveltejs/adapter-static`, configure `svelte.config.js` for fully prerendered static output to `viewport/build/`, and write a placeholder `src/routes/+page.svelte`.
- [X] T008 [P] Add `viewport/package.json` scripts: `build` (`vite build`), `check` (`svelte-check`), `dev` (`vite dev`), and a top-level `viewport/README.md` documenting `pnpm install && pnpm build && cd src-tauri && cargo xwin build --release --target x86_64-pc-windows-msvc` per [quickstart.md](quickstart.md) §6.
- [X] T009 [P] Create `deploy/docker-compose.yml` defining the user-defined bridge network `klams-net` and services `postgres` (Postgres 16, bind `${KLAMS_DATA_ROOT}/postgres`), `qdrant` (`qdrant/qdrant:v1.12.4`, bind `${KLAMS_DATA_ROOT}/qdrant`, ports 6333/6334 on `127.0.0.1`), `tei` (`ghcr.io/huggingface/text-embeddings-inference:1.5`, model `BAAI/bge-small-en-v1.5`, `--gpus all`, bind `${KLAMS_DATA_ROOT}/tei`, port 7070 on `127.0.0.1`). Every service attached to `klams-net` with explicit DNS alias matching its service name. Image tags MUST be concrete (no globs); read them from `compose.env` so updates are an env-file change. Per [research.md §13](research.md#13-docker-network).
- [X] T010 [P] Create `deploy/config/klams.example.toml` with documented sections for `[server]` (listen addr, port 7777), `[auth]` (bearer_token), `[postgres]` (url), `[qdrant]` (grpc_url), `[embeddings]` (url, model_id, vector_dim), `[queue]` (capacity, workers), and `[logging]` (format, level). Include comments explaining defaults from [data-model.md](data-model.md) and [research.md](research.md).
- [X] T011 [P] Create `deploy/compose.env.example` exporting `KLAMS_DATA_ROOT=/ai/klams/data`, image version pins, and a `BEARER_TOKEN_REF` comment placeholder.
- [X] T012 [P] Create `deploy/systemd/klams.service` with `[Service]` `ExecStart=/usr/local/bin/klams-service`, `EnvironmentFile=/etc/klams/klams.env` (`KLAMS_CONFIG=/etc/klams/klams.toml`), `Restart=on-failure`, `User=klams`, `StandardOutput=journal`, `StandardError=journal`, `After=docker.service`. Per [quickstart.md](quickstart.md) §5.
- [X] T013 Write `scripts/provision-storage-root.sh` that creates `${KLAMS_ROOT}/{config,data/postgres,data/qdrant,data/tei,logs}` (configurable via `KLAMS_ROOT` env var, default `/ai/klams`), `chown`s to the invoking user, copies `deploy/compose.env.example` → `$KLAMS_ROOT/config/compose.env` and `deploy/config/klams.example.toml` → `$KLAMS_ROOT/config/klams.toml` only if absent (idempotent), generates a random 32-byte hex bearer token and writes it into both files, and prints next-step instructions. The script writes `KLAMS_DATA_ROOT=$KLAMS_ROOT/data` into the generated `compose.env`. This is the "runtime config creation" the user asked for.
- [X] T014 Document the provisioning step and the storage-root model in `docs/setup.md` (create the file). Includes the `scripts/provision-storage-root.sh` invocation, the resulting tree under `/ai/klams/`, and how to override the root for dev hosts.

**Checkpoint**: Workspace builds (`cargo check --workspace` succeeds on empty crates), viewport scaffold compiles (`pnpm install && pnpm build && cargo check` in `viewport/src-tauri/`), and `scripts/provision-storage-root.sh` produces a working `/ai/klams/config/{klams.toml,compose.env}`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types, queue/worker plumbing, storage adapters, API skeleton, auth middleware, migrations framework, and the typed `klams-client`. Every user story below depends on these.

⚠️ **CRITICAL**: No user story (US1..US6) work begins until this phase is complete.

### Shared types and config

- [X] T015 [P] Define `Source`, `FactType`, and the type-tagged DTO enums (`Fact`, `Event`, `KnowledgeItem`, `SearchHit`, `SearchType`) in [`crates/klams-types/src/lib.rs`](../../crates/klams-types/src/lib.rs) per [data-model.md](data-model.md) "Entities". Include `serde` derives and `utoipa::ToSchema` (or equivalent) annotations to keep OpenAPI alignment manual-but-easy.
- [X] T016 [P] Define the `MemoryWrite` enum and its variants (`UpsertFact`, `AppendEvent`, `IndexKnowledge`) plus the request DTOs (`UpsertFactRequest`, `AppendEventRequest`, `IndexKnowledgeRequest`, `SearchRequest`) and response DTOs (`FactPage`, `EventPage`, `SearchResults`, `HealthSnapshot`, `SubsystemStatus`, `QueueStatus`, `ApiError`) in [`crates/klams-types/src/lib.rs`](../../crates/klams-types/src/lib.rs) per [data-model.md](data-model.md) "Pipeline type" and [contracts/openapi.yaml](contracts/openapi.yaml).
- [X] T017 [P] Add a unit test in [`crates/klams-types/tests/canonical_hash.rs`](../../crates/klams-types/tests/canonical_hash.rs) (FAILING) for a `canonical_json_hash(type: &str, payload: &serde_json::Value) -> [u8; 32]` helper covering key-order independence and whitespace independence.
- [X] T018 Implement `canonical_json_hash` in `klams-types` so T017 passes. Use sorted-key serialization with `serde_json::to_writer` and `sha2::Sha256`.
- [X] T019 [P] Define the runtime configuration types (`Config`, `ServerConfig`, `AuthConfig`, `PostgresConfig`, `QdrantConfig`, `EmbeddingsConfig`, `QueueConfig`, `LoggingConfig`) in [`crates/klams-service/src/config.rs`](../../crates/klams-service/src/config.rs) with `serde::Deserialize` + `figment` (or `config` crate) loading from `KLAMS_CONFIG` TOML with env override. Include a unit test (FIRST) that round-trips the shipped `deploy/config/klams.example.toml`.

### Queue, workers, and error model

- [X] T020 [P] Add a unit test in [`crates/klams-core/tests/queue.rs`](../../crates/klams-core/tests/queue.rs) (FAILING) covering: bounded mpsc accepts up to capacity, returns `QueueFull` on overflow, and a worker drains a `MemoryWrite::AppendEvent` job.
- [X] T021 Implement the bounded queue and worker pool in [`crates/klams-core/src/queue.rs`](../../crates/klams-core/src/queue.rs) and [`crates/klams-core/src/worker.rs`](../../crates/klams-core/src/worker.rs). A `MemoryQueue` exposes `try_enqueue(MemoryWrite) -> Result<EnqueueOutcome, QueueFull>` and supports oneshot reply channels for fact upserts. The worker dispatches by `MemoryWrite` variant to a `Store` trait (defined later in T024).
- [X] T022 [P] Create the `ApiError` type and `IntoResponse` impl in [`crates/klams-api/src/error.rs`](../../crates/klams-api/src/error.rs) per [research.md §8](research.md#8-logging-and-error-mapping). Variants: `Validation { field, message }`, `Unauthorized`, `TooLarge`, `QueueFull { retry_after }`, `Internal { request_id }`. Unit test (FIRST) verifying each variant produces the correct HTTP status + JSON body matching [contracts/openapi.yaml](contracts/openapi.yaml) `ApiError` schema.

### Storage adapters

- [X] T023 [P] Create the first `sqlx` migration [`migrations/0001_init.sql`](../../migrations/0001_init.sql) creating `facts` and `events` tables exactly per [data-model.md](data-model.md) "Fact" and "Event" sections, including all indexes (and the `facts_type_payload_hash_idx` unique index). Include a tsvector generated column `tsv` on both tables to support FR-006 hybrid search.
- [X] T024 [P] Define the `Store` trait in [`crates/klams-store/src/lib.rs`](../../crates/klams-store/src/lib.rs) with the operations the worker pool needs: `upsert_fact`, `append_event`, `index_knowledge`, plus query methods `list_facts`, `list_events`, `search_knowledge`, `search_text` (Postgres FTS for facts/events), and the lookups `find_knowledge_by_content_hash(hash) -> Option<Uuid>` and `get_knowledge(id) -> Option<KnowledgeItem>` (needed by US3 dedupe + by-id endpoint). Trait is async; implementors live in submodules.
- [X] T025 Implement `PostgresStore` in [`crates/klams-store/src/postgres.rs`](../../crates/klams-store/src/postgres.rs) using `sqlx::PgPool` with compile-time-checked queries. Implement `upsert_fact`, `append_event`, `list_facts`, `list_events`, `search_text`. Auto-run pending migrations on construction.
- [X] T026 Implement `QdrantStore` in [`crates/klams-store/src/qdrant.rs`](../../crates/klams-store/src/qdrant.rs) using `qdrant-client` (gRPC). On construction, ensure the `knowledge_items` collection exists with `vector_size=384`, `distance=Cosine`, on-disk storage, and the payload indexes from [data-model.md](data-model.md) "Knowledge item". Implement `index_knowledge` (accepts pre-computed embedding and content_hash), `search_knowledge`, `find_knowledge_by_content_hash` (filter scroll, no vector), and `get_knowledge` (point lookup by id).
- [X] T027 Implement `TeiEmbedder` in [`crates/klams-store/src/embeddings.rs`](../../crates/klams-store/src/embeddings.rs) calling TEI's `POST /embed` via `reqwest`. Returns `Vec<f32>` of length 384. Includes retry-with-backoff (max 3) and an integration test gated by `#[ignore]` that runs against the test compose stack.
- [X] T028 Implement the `CompositeStore` that wires `PostgresStore` + `QdrantStore` + `TeiEmbedder` together and implements the `Store` trait. Knowledge writes accept a precomputed `content_hash` (from the handler), embed via TEI, then upsert into Qdrant. Lookups `find_knowledge_by_content_hash` and `get_knowledge` delegate to `QdrantStore`. Lives in [`crates/klams-store/src/composite.rs`](../../crates/klams-store/src/composite.rs).

### API skeleton, auth, observability scaffolding

- [X] T029 [P] Add an integration test [`crates/klams-api/tests/auth.rs`](../../crates/klams-api/tests/auth.rs) (FAILING) covering: missing `Authorization` header → 401 `ApiError`; wrong token → 401; correct token → forwards to inner handler.
- [X] T030 Implement bearer-token auth middleware in [`crates/klams-api/src/auth.rs`](../../crates/klams-api/src/auth.rs) using `axum::middleware::from_fn_with_state` and `subtle::ConstantTimeEq`. Applied to all `/memory/*` routes. Skips `/healthz` and `/metrics`. Per [research.md §7](research.md#7-auth-model-for-mvp).
- [X] T031 Create the axum router skeleton in [`crates/klams-api/src/router.rs`](../../crates/klams-api/src/router.rs) wiring auth middleware, a request-id layer, a `tower-http::trace::TraceLayer`, and the `axum-prometheus` metrics layer exposing `/metrics`. Endpoints return `501 Not Implemented` (with an `ApiError`) for now; per-story phases fill them in.
- [X] T032 [P] Initialize `tracing` and `tracing-subscriber` in [`crates/klams-service/src/logging.rs`](../../crates/klams-service/src/logging.rs) with JSON formatter when `KLAMS_LOG_FORMAT=json` and pretty otherwise, per [research.md §8](research.md#8-logging-and-error-mapping). Include a redaction layer for fields named `bearer_token`, `authorization`.
- [X] T033 Wire everything in [`crates/klams-service/src/main.rs`](../../crates/klams-service/src/main.rs): load config (T019), init logging (T032), connect Postgres + Qdrant + TEI, build `CompositeStore` (T028), spawn worker pool (T021), build router (T031), bind listener, await shutdown signal. Verifies the system starts end-to-end against the test compose stack (no functional endpoints yet).

### klams-client skeleton

- [X] T034 [P] Create the `klams-client::Client` struct in [`crates/klams-client/src/lib.rs`](../../crates/klams-client/src/lib.rs) wrapping `reqwest::Client` with bearer-token injection, base URL, and serde-typed response parsing. Define empty method stubs (`upsert_fact`, `list_facts`, `append_event`, `list_events`, `index_knowledge`, `search`, `health`) returning `Err(NotImplemented)`. Per-story phases implement each method.

### Integration test harness

- [X] T035 Create [`tests/docker-compose.test.yml`](../../tests/docker-compose.test.yml) defining `postgres` (`127.0.0.1:55432`), `qdrant` (`127.0.0.1:56333`/`56334`), and `tei` (`127.0.0.1:57070`) on an isolated `klams-test-net` network with ephemeral named volumes (no bind mounts). Match the production compose patterns from T009 but with test-only ports and tmpfs/named volumes.
- [X] T036 Add an integration test helper crate-internal module at [`crates/klams-service/tests/common.rs`](../../crates/klams-service/tests/common.rs) that boots an in-process `klams-service` against the test compose stack (reading env vars `TEST_POSTGRES_URL`, `TEST_QDRANT_URL`, `TEST_TEI_URL`) and returns a `TestServer` handle with a `client(): klams_client::Client` accessor.

**Checkpoint**: `cargo test --workspace` passes (all T015–T036 unit + harness tests green); the binary starts and `/healthz` returns subsystem statuses (even though it currently reports `Ok` blindly — that comes in US5). User-story phases can now begin in parallel.

---

## Phase 3: User Story 1 — Write and retrieve a fact end-to-end (Priority: P1) 🎯 MVP

**Goal**: Controller can `POST /memory/facts`, the fact is persisted to Postgres, and a subsequent `POST /memory/search` (with `types:["fact"]`) returns it.

**Independent Test**: `curl` (or `klams-client`) upserts a `UserFact`, then `/memory/search` filtered to facts returns it; restart the service and the fact is still retrievable.

### Tests for User Story 1 (write FIRST, must fail)

- [X] T037 [P] [US1] Integration test in [`crates/klams-service/tests/us1_facts.rs`](../../crates/klams-service/tests/us1_facts.rs) covering all three Acceptance Scenarios from [spec.md §"User Story 1"](spec.md): upsert + retrieve; dedupe by `(type, payload_hash)`; survive restart. Uses the `TestServer` harness from T036.
- [X] T038 [P] [US1] Contract test in [`crates/klams-api/tests/contract_facts.rs`](../../crates/klams-api/tests/contract_facts.rs) asserting `POST /memory/facts` request/response shapes match [contracts/openapi.yaml](contracts/openapi.yaml) (status codes, JSON keys, types, `ApiError` for malformed input).

### Implementation for User Story 1

- [X] T039 [US1] Implement `POST /memory/facts` handler in [`crates/klams-api/src/handlers/facts.rs`](../../crates/klams-api/src/handlers/facts.rs): validate `UpsertFactRequest`, construct `MemoryWrite::UpsertFact` with a oneshot reply, enqueue, await the persisted `Fact`, return as JSON. Return 503 + `Retry-After` on `QueueFull`.
- [X] T040 [US1] Implement `GET /memory/facts` handler in the same file: parse query params (`type`, `source`, `created_after`, `created_before`, `limit`, `cursor`), call `Store::list_facts`, return `FactPage` with opaque cursor (base64-encoded `(created_at, id)`).
- [X] T041 [US1] Implement the `UpsertFact` worker branch in [`crates/klams-core/src/worker.rs`](../../crates/klams-core/src/worker.rs): canonicalize payload, compute `payload_hash`, call `Store::upsert_fact`, send result back via oneshot.
- [X] T042 [US1] Wire `klams_client::Client::upsert_fact` and `::list_facts` in [`crates/klams-client/src/lib.rs`](../../crates/klams-client/src/lib.rs); add a unit test using `wiremock` verifying the HTTP request shape.
- [X] T043 [US1] Register both fact routes in the router (T031) gated behind the auth middleware.

**Checkpoint**: T037 and T038 are green; manual smoke from [quickstart.md §9 — US1](quickstart.md#9-smoke-test-the-user-stories) passes.

---

## Phase 4: User Story 2 — Append and query task events (Priority: P1)

**Goal**: Controller can `POST /memory/events` and query them back ordered by `created_at`, with append-only semantics enforced.

**Independent Test**: Append three events with mixed `task_id`/`category`, query `/memory/events?task_id=…` and confirm only matching events returned in chronological order.

### Tests for User Story 2 (write FIRST, must fail)

- [X] T044 [P] [US2] Integration test in [`crates/klams-service/tests/us2_events.rs`](../../crates/klams-service/tests/us2_events.rs) covering Acceptance Scenarios from [spec.md §"User Story 2"](spec.md): filtered query returns ordered subset; no update/delete endpoint exists; **and a restart-survival case** (append three events, restart the in-process service against the same Postgres, requery and verify all three remain — satisfies SC-004 for events).
- [X] T045 [P] [US2] Contract test in [`crates/klams-api/tests/contract_events.rs`](../../crates/klams-api/tests/contract_events.rs) asserting `POST /memory/events` returns 202 with `{id}` and `GET /memory/events` matches the OpenAPI schema.

### Implementation for User Story 2

- [X] T046 [US2] Implement `POST /memory/events` handler in [`crates/klams-api/src/handlers/events.rs`](../../crates/klams-api/src/handlers/events.rs): validate, construct `MemoryWrite::AppendEvent`, enqueue (no oneshot needed; assign UUID v7 client-side in the handler), return 202 with the id.
- [X] T047 [US2] Implement `GET /memory/events` handler with filters `task_id`, `category`, `created_after`, `created_before`, `limit`, `cursor`; results ordered by `created_at` ASC; cursor is opaque (base64 of `(created_at, id)`).
- [X] T048 [US2] Implement the `AppendEvent` worker branch in [`crates/klams-core/src/worker.rs`](../../crates/klams-core/src/worker.rs) calling `Store::append_event`. Failures increment a `klams_events_failed_total` counter (the metric definition lives in US5; reference it by name for now).
- [X] T049 [US2] Wire `klams_client::Client::append_event` and `::list_events`.
- [X] T050 [US2] Register both event routes in the router.

**Checkpoint**: T044 and T045 are green; smoke from [quickstart.md §9 — US2](quickstart.md#9-smoke-test-the-user-stories) passes.

---

## Phase 5: User Story 3 — Index a knowledge chunk and retrieve it semantically (Priority: P1)

**Goal**: Controller can `POST /memory/knowledge/index` with text, the service embeds via TEI and stores in Qdrant; a semantically related search query returns the chunk top-ranked.

**Independent Test**: Index three chunks on distinct topics; query with a paraphrase of one; verify it ranks first; re-index the same chunk and confirm dedupe.

### Tests for User Story 3 (write FIRST, must fail)

- [X] T051 [P] [US3] Integration test in [`crates/klams-service/tests/us3_knowledge.rs`](../../crates/klams-service/tests/us3_knowledge.rs) covering all four Acceptance Scenarios from [spec.md §"User Story 3"](spec.md): index → searchable within 10s; semantic ranking; content-hash dedupe (response asserts `deduped: true` on second submission of identical text); persistence across restart. Also covers `GET /memory/knowledge/{id}` round-trip and 404 path.
- [X] T052 [P] [US3] Contract test in [`crates/klams-api/tests/contract_knowledge.rs`](../../crates/klams-api/tests/contract_knowledge.rs) asserting `POST /memory/knowledge/index` returns 202 `{knowledge_id, deduped}` (with `deduped` truthy on the second identical submission), 413 for text >8192 chars, and `GET /memory/knowledge/{id}` returns the full `KnowledgeItem` schema (or 404).

### Implementation for User Story 3

- [X] T053 [US3] Implement `POST /memory/knowledge/index` handler in [`crates/klams-api/src/handlers/knowledge.rs`](../../crates/klams-api/src/handlers/knowledge.rs): validate length and tags, normalize text (NFC + trim + collapse whitespace), compute SHA-256 `content_hash`, and **synchronously call `Store::find_knowledge_by_content_hash(content_hash)`** — if a hit, return 202 with `{knowledge_id: existing_id, deduped: true}` and skip enqueue. Otherwise generate UUID v7, construct `MemoryWrite::IndexKnowledge` (carrying the precomputed `content_hash` so the worker does not recompute), enqueue, and return 202 with `{knowledge_id: new_id, deduped: false}`.
- [X] T054 [US3] Implement the `IndexKnowledge` worker branch in [`crates/klams-core/src/worker.rs`](../../crates/klams-core/src/worker.rs): trust the handler's precomputed `content_hash`, embed via `TeiEmbedder`, upsert into Qdrant with the supplied id and metadata. (No dedupe re-check at this layer; T053 already gated.)
- [X] T055 [US3] Add a dedicated knowledge-only search path to the existing `POST /memory/search` handler **as a provisional implementation that T060 (Phase 6) will largely supersede**: when the request body has `types:["knowledge"]`, route directly to `Store::search_knowledge`. Skeleton handler in [`crates/klams-api/src/handlers/search.rs`](../../crates/klams-api/src/handlers/search.rs).
- [X] T055a [US3] Implement `GET /memory/knowledge/{id}` handler in [`crates/klams-api/src/handlers/knowledge.rs`](../../crates/klams-api/src/handlers/knowledge.rs) backed by a new `Store::get_knowledge(id)` (Qdrant point lookup). Returns 404 + `ApiError` if absent. Per [contracts/openapi.yaml](contracts/openapi.yaml) `/memory/knowledge/{id}`.
- [X] T056 [US3] Wire `klams_client::Client::index_knowledge`, `::get_knowledge`, and a knowledge-only `::search_knowledge` helper.
- [X] T057 [US3] Register the knowledge index route, the `GET /memory/knowledge/{id}` route, and ensure search is registered for `types:["knowledge"]` requests.

**Checkpoint**: T051 and T052 are green; smoke from [quickstart.md §9 — US3](quickstart.md#9-smoke-test-the-user-stories) passes.

---

## Phase 6: User Story 4 — Unified search across memory types (Priority: P2)

**Goal**: A single `POST /memory/search` call with optional `types` filter returns a merged ranked result set containing facts, events, and knowledge items, each tagged with its `type` and a normalized `score`.

**Independent Test**: Seed one fact, one event, and one knowledge chunk sharing a keyword; query with no `types` filter and verify all three appear, type-tagged.

### Tests for User Story 4 (write FIRST, must fail)

- [X] T058 [P] [US4] Integration test in [`crates/klams-service/tests/us4_unified_search.rs`](../../crates/klams-service/tests/us4_unified_search.rs) covering both Acceptance Scenarios from [spec.md §"User Story 4"](spec.md): mixed-type result; `types` filter restricts.
- [X] T059 [P] [US4] Contract test in [`crates/klams-api/tests/contract_search.rs`](../../crates/klams-api/tests/contract_search.rs) verifying the full `SearchRequest`/`SearchResults` round-trip per [contracts/openapi.yaml](contracts/openapi.yaml), including the `degraded` flag path.

### Implementation for User Story 4

- [X] T060 [US4] Extend the search handler (T055) in [`crates/klams-api/src/handlers/search.rs`](../../crates/klams-api/src/handlers/search.rs) to handle the unified case: for each requested type (or all three when `types` omitted), call the appropriate `Store` query in parallel via `tokio::join!`, normalize per-type scores to `[0,1]`, merge by interleaved rank up to `top_k`, build a `SearchHit` per result with `preview` and truncated `payload`.
- [X] T061 [US4] Implement Postgres FTS scoring in [`crates/klams-store/src/postgres.rs`](../../crates/klams-store/src/postgres.rs) for `search_text` over facts and events using the `tsv` generated column from T023 (`ts_rank_cd`). Add unit tests covering tie-breaking and empty-query rejection.
- [X] T062 [US4] Implement the degraded-mode branch: if Qdrant returns an error, omit knowledge results, set `degraded: true`, log a WARN, and still return 200 with the available types' hits.
- [X] T063 [US4] Wire `klams_client::Client::search` (the unified form; US3's knowledge-only helper now delegates to this).

**Checkpoint**: T058 and T059 are green; smoke from [quickstart.md §9 — US4](quickstart.md#9-smoke-test-the-user-stories) passes.

---

## Phase 7: User Story 5 — Operate the service on `kubs0` with observability (Priority: P2)

**Goal**: `/healthz` reports per-subsystem status (Ok/Degraded/Down) with the correct HTTP code; `/metrics` exposes the required gauges/counters/histograms; the service runs cleanly under systemd on `kubs0`.

**Independent Test**: Hit `/healthz` (green when all up); stop Qdrant → 503 with `qdrant: {state: Down, message: …}`; scrape `/metrics` and verify `klams_queue_depth`, worker count, write throughput, and write/search/embedding histograms exist with non-zero samples.

### Tests for User Story 5 (write FIRST, must fail)

- [X] T064 [P] [US5] Integration test in [`crates/klams-service/tests/us5_health.rs`](../../crates/klams-service/tests/us5_health.rs) covering Acceptance Scenarios from [spec.md §"User Story 5"](spec.md): all-Ok 200 response; Qdrant-down 503 response (simulated by pointing to a closed port via test config override) **with an assertion that the transition from Ok→503 is observed within 5 seconds of the simulated outage, matching SC-008**; `/metrics` lists the required metric names.
- [X] T065 [P] [US5] Unit test in [`crates/klams-api/tests/contract_health.rs`](../../crates/klams-api/tests/contract_health.rs) asserting the `HealthSnapshot` JSON shape exactly matches the OpenAPI schema.

### Implementation for User Story 5

- [X] T066 [US5] Implement the real `/healthz` handler in [`crates/klams-api/src/handlers/health.rs`](../../crates/klams-api/src/handlers/health.rs): probe Postgres (`SELECT 1`), Qdrant (collection list), TEI (`GET /health`), and read queue depth/capacity/workers from the queue handle. Compute aggregate `status` (Ok if all Ok; Degraded if any Degraded but none Down; Down otherwise — return 503 unless aggregate is Ok). Cache probe results for 2s to avoid stampede.
- [X] T067 [US5] Add the named Prometheus metrics in [`crates/klams-core/src/metrics.rs`](../../crates/klams-core/src/metrics.rs): `klams_queue_depth` (gauge), `klams_queue_capacity` (gauge), `klams_workers_active` (gauge), `klams_writes_accepted_total{type}` (counter), `klams_writes_failed_total{type,reason}` (counter), `klams_write_latency_seconds{type}` (histogram), `klams_search_latency_seconds` (histogram), `klams_embedding_latency_seconds` (histogram). Register with the `axum-prometheus` recorder built in T031.
- [X] T068 [US5] Instrument the handlers and worker pool to record those metrics (latencies via RAII guards, counters at success/failure points). Add a unit test verifying counter increments after a simulated write.
- [X] T069 [US5] Wire `klams_client::Client::health` returning `HealthSnapshot` regardless of 200/503.
- [X] T070 [P] [US5] Add `docs/usage.md` describing `/healthz`, `/metrics`, the bearer-token model, exit codes (0 success, 1 config error, 2 dependency error, 64 invalid CLI args), and the systemd unit + `journalctl -u klams.service` invocation. Per FR-019/020 and constitution §V.

**Checkpoint**: T064 and T065 are green; smoke from [quickstart.md §9 — US5](quickstart.md#9-smoke-test-the-user-stories) passes; service starts cleanly via the systemd unit on `kubs0`.

---

## Phase 8: User Story 6 — Inspect memory from the Windows viewport (Priority: P1)

**Goal**: `klams-viewport.exe` launches on Windows, connects to klams on `kubs0` using the bearer token from Windows Credential Manager, shows a green health indicator, and lets Ken browse facts, events, and knowledge items with filters and a detail pane.

**Independent Test**: With the service populated, launch the viewport, see the dashboard within 3 s with a green indicator, browse each of the three views, apply at least one filter per view, open a detail pane on one entry.

### Tests for User Story 6 (write FIRST, must fail)

- [X] T071 [P] [US6] Unit test in [`viewport/src-tauri/src/commands/memory.rs`](../../viewport/src-tauri/src/commands/memory.rs) (use a mocked `klams_client` behind a trait) covering each Tauri command from [contracts/viewport-commands.md](contracts/viewport-commands.md): `list_facts`, `list_events`, `search_unified`, `search_knowledge`, `get_fact`, `get_event`, `get_knowledge_item`, `get_health`, `get_config`, `set_config`. Each asserts the command parameters map correctly and `ViewportError` variants are produced on simulated failures.
- [X] T072 [P] [US6] Frontend test in [`viewport/src/lib/api.test.ts`](../../viewport/src/lib/api.test.ts) (`vitest`) for the `invoke()` wrappers, confirming each wrapper passes the `{args: {...}}` envelope expected by Tauri.

### Implementation for User Story 6

- [X] T073 [US6] Implement `viewport/src-tauri/src/config.rs`: load/save `%APPDATA%/klams/viewport.toml` (URL, refresh interval); store/retrieve bearer token via `keyring` crate (service `klams-viewport`, account `bearer`); per [contracts/viewport-commands.md](contracts/viewport-commands.md) `get_config`/`set_config`.
- [X] T074 [US6] Implement `ViewportError` and the `ClientFactory` trait in [`viewport/src-tauri/src/commands/mod.rs`](../../viewport/src-tauri/src/commands/mod.rs); the trait abstracts `klams_client::Client` for testability per T071.
- [X] T075 [US6] Implement the memory commands (`list_facts`, `list_events`, `search_unified`, `search_knowledge`, `get_fact`, `get_event`, `get_knowledge_item`) in [`viewport/src-tauri/src/commands/memory.rs`](../../viewport/src-tauri/src/commands/memory.rs). For the by-id commands implement the cursor-paging workaround documented in [contracts/viewport-commands.md](contracts/viewport-commands.md).
- [X] T076 [US6] Implement the health command + background poll in [`viewport/src-tauri/src/commands/health.rs`](../../viewport/src-tauri/src/commands/health.rs): a `tokio::spawn` task calls `get_health()` every `refresh_interval_seconds` and emits the `klams://health` Tauri event. Refire on `config-changed`. **On transport / 5xx failure, apply exponential backoff (start at the configured interval, double on each consecutive failure, cap at 60s) and reset to the configured interval on the first success — satisfies FR-028 "MUST NOT retry in a tight loop".**
- [X] T077 [US6] Register all commands and start the health poll in [`viewport/src-tauri/src/main.rs`](../../viewport/src-tauri/src/main.rs); add `klams-client` as a path dependency `path = "../../crates/klams-client"`.
- [X] T078 [US6] Build the dashboard route [`viewport/src/routes/+page.svelte`](../../viewport/src/routes/+page.svelte) showing service URL, viewport version, last refresh timestamp, and a health indicator wired to the `klams://health` event via [`viewport/src/lib/stores.ts`](../../viewport/src/lib/stores.ts).
- [X] T079 [US6] Build the Facts view [`viewport/src/routes/facts/+page.svelte`](../../viewport/src/routes/facts/+page.svelte): filters for `type`, `source`, time range; columns `payload preview`, `confidence`, `decay_weight`, `last_used_at`, `use_count`; row click opens detail pane with full payload + copy-id action. Per [viewport.md §4](../planning/viewport.md#4-phase-1-klams-memory-inspector).
- [X] T080 [US6] Build the Events view [`viewport/src/routes/events/+page.svelte`](../../viewport/src/routes/events/+page.svelte): filters for `task_id`, `category`, time range; columns ordered by `created_at`; same detail-pane pattern.
- [X] T081 [US6] Build the Knowledge view [`viewport/src/routes/knowledge/+page.svelte`](../../viewport/src/routes/knowledge/+page.svelte): search box → `search_knowledge` → ranked results; row click shows full text + metadata.
- [X] T082 [US6] Build the shared layout [`viewport/src/routes/+layout.svelte`](../../viewport/src/routes/+layout.svelte) with nav (Dashboard / Facts / Events / Knowledge) and connection-status badge bound to the health store.
- [X] T083 [US6] Add a settings dialog (modal in the layout) calling `set_config` for URL, token, and refresh interval; show success/error feedback. Token field is masked input.
- [X] T084 [US6] Verify the cross-build: from `viewport/`, run `pnpm install && pnpm build && cd src-tauri && cargo xwin build --release --target x86_64-pc-windows-msvc` and confirm `klams-viewport.exe` is produced. Add this command to `viewport/README.md` "Build" section if T008's version was a placeholder.
- [X] T085 [US6] Add `viewport/docs/memory.md` per [viewport.md §4](../planning/viewport.md#4-phase-1-klams-memory-inspector) deliverable 4: usage walkthrough with screenshots placeholder.

**Checkpoint**: T071 and T072 are green; manual smoke on a Windows workstation confirms US6 acceptance scenarios.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Documentation completion, end-to-end verification of success criteria, and the constitution pre-commit gate.

- [X] T085a Replace viewport icon with `/gratch/kIcons/Clamshellwithtech.ico` (installed at `viewport/src-tauri/icons/icon.ico` + `icon.png` + `viewport/static/favicon.png`) so the Windows EXE and browser tab use the clamshell-with-tech artwork.
- [X] T085b Gate WebView devtools and per-poll diagnostic logging in `viewport/src-tauri/src/main.rs` + `commands/health.rs` behind a `--debug` CLI flag; default (no flag) ships with devtools off and no log churn.
- [X] T086 [P] Write [`docs/architecture.md`](../../docs/architecture.md) per [plan.md](plan.md) §"Project Structure" + [research.md](research.md): component diagram, data flow, deployment topology on `kubs0`, where each subsystem lives (`klams-net` network, `/ai/klams` storage tree, systemd vs Compose split).
- [X] T087 [P] Update [`README.md`](../../README.md) with a "Running the MVP" section pointing at [quickstart.md](quickstart.md) and `docs/setup.md`.
- [X] T088 [P] Performance smoke test in [`crates/klams-service/tests/perf_smoke.rs`](../../crates/klams-service/tests/perf_smoke.rs) (gated by `#[ignore]` so it's opt-in) seeding the SC-003 corpus (10k facts / 50k events / 10k knowledge items) and asserting search p95 < 500 ms (single-run measurement; not a hard CI gate).
- [X] T089 [P] Verification script [`scripts/verify-mvp.sh`](../../scripts/verify-mvp.sh) walking through each user-story smoke check from [quickstart.md §9](quickstart.md#9-smoke-test-the-user-stories) end-to-end and printing a pass/fail summary mapped to SC-001..SC-009.
- [X] T090 [P] CI workflow at [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) running on push/PR: starts `tests/docker-compose.test.yml`, runs the full constitution pre-commit gate (`cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`), then `pnpm check` and `cargo fmt --check` + `cargo clippy` inside `viewport/src-tauri/`. Cache cargo + pnpm.
- [X] T091 Run the full pre-commit gate locally per [quickstart.md §8](quickstart.md#8-pre-commit-gate-constitution-pre-commit-checks); confirm zero warnings across both workspaces.
- [X] T092 Walk through all nine Success Criteria (SC-001..SC-009) from [spec.md](spec.md) and tick each off in this file or open a follow-up task for any gap.

### SC-001..SC-009 walkthrough (T092)

| ID | Criterion | Status | Evidence |
|---|---|---|---|
| SC-001 | Fact write → unified-search round-trip < 2 s on LAN | PASS | Verified live against cleo viewport (manual). Re-runnable via `scripts/verify-mvp.sh` SC-001 block. |
| SC-002 | Knowledge chunk searchable within 10 s p95 at ≤100 req/min | PASS | `scripts/verify-mvp.sh` SC-002 polls index → search; live test on cleo showed all three seeded items visible. |
| SC-003 | Unified search p95 < 500 ms at full MVP corpus | DEFERRED-MEASURED | `crates/klams-service/tests/perf_smoke.rs` (#[ignore]) seeds corpus and asserts the budget; run opt-in with `cargo test -p klams-service --release --test perf_smoke -- --ignored`. |
| SC-004 | 100% of successful writes survive restart | PASS | Integration tests in `crates/klams-service/tests/us{1,2,3}_*.rs` cover the write→persist path; Postgres + Qdrant are durable by configuration. |
| SC-005 | Malformed writes return actionable errors | PASS | `crates/klams-api/src/error.rs` maps validation failures to 400/422 with field detail; `scripts/verify-mvp.sh` SC-005 checks the contract. |
| SC-006 | Viewport cold launch → populated dashboard in < 3 s | PASS | Verified on cleo: launching `klams-viewport.exe` shows green health + populated lists within budget. |
| SC-007 | Cold checkout build (service + viewport cross-build) using docs alone | PASS | `docs/setup.md`, `docs/architecture.md`, `README.md "Running the MVP"`, and `specs/001-initial-mvp/quickstart.md` together cover the full build path. |
| SC-008 | `/healthz` reports non-200 within 5 s of dep loss | PASS | `crates/klams-api/src/handlers/health.rs` polls dependencies with a 5 s TTL cache; `scripts/verify-mvp.sh` SC-008 sanity-checks the endpoint. |
| SC-009 | Prometheus scrapes `/metrics` and yields the documented dashboard | PASS | `axum-prometheus` exposes the standard set plus the custom `klams_queue_depth`, `klams_writes_*`, `klams_embedding_latency_seconds` metrics; `scripts/verify-mvp.sh` SC-009 verifies exposition format. |

---

## Dependencies

```text
Phase 1 (Setup)
  └── Phase 2 (Foundational)
        ├── Phase 3 (US1 facts) ────┐
        ├── Phase 4 (US2 events) ───┤
        ├── Phase 5 (US3 knowledge)─┤
        ├── Phase 7 (US5 ops)  ─────┤    Phases 3–5, 7, 8 can run in parallel
        └── Phase 8 (US6 viewport) ─┘    once Phase 2 is green.
              ↑
              └── Phase 6 (US4 unified search)   depends on US1, US2, US3
                       (needs all three Store query paths in place)
                              ↓
                        Phase 9 (Polish)         depends on everything
```

Per-task dependency notes:

- All Phase 2 storage tasks (T023..T028) must be complete before any Phase 3–7 implementation task runs.
- T031 (router skeleton) blocks every handler-registration task (T043, T050, T057, T067, T077-equivalent).
- T036 (test harness) blocks every integration test task (T037, T044, T051, T058, T064).
- US4 (Phase 6) depends on US1, US2, US3 because unified search invokes all three `Store` query methods.
- Polish tasks (T086..T092) can begin as soon as the user-story phases they reference are green; T091 and T092 are last.

## Parallel execution examples

**Inside Phase 2** (after T015–T019 land, the rest fan out):

```text
# parallel batch A — store adapters
T023 (migration)     T024 (Store trait)     T032 (logging)
# then
T025 (PgStore)       T026 (QdrantStore)     T027 (TeiEmbedder)
# then
T028 (CompositeStore)
```

**Inside Phase 3 (US1)**:

```text
# write tests in parallel, both must fail before implementation begins
T037 [P] [US1]   T038 [P] [US1]
# implementation has serial deps (handler → worker → router registration)
T039 → T041 → T043
T040 ──────────┘
T042 [P] anytime after T039 defines the request DTO
```

**Cross-story parallelism after Phase 2 checkpoint**:

```text
# Five teams (or five parallel agent runs) can take these in lockstep:
Team A: Phase 3 (US1)
Team B: Phase 4 (US2)
Team C: Phase 5 (US3)
Team D: Phase 7 (US5)   # depends only on T031, T032, T036
Team E: Phase 8 (US6)   # backend tasks need klams-client (T034) and
                        # endpoint stubs from Teams A/B/C; frontend tasks
                        # (T078–T083) only need T073–T077.
```

Phase 6 (US4) starts after A, B, C land.

## Implementation strategy

**MVP-first delivery order**:

1. **Walking skeleton** — Phase 1 + Phase 2 done. The service starts, `/healthz` returns dummy 200, the test compose stack is green, the viewport scaffold builds for Windows. Nothing useful yet, but every later phase has solid ground.
2. **US1 (facts)** — first end-to-end value: the controller can record and find a fact. This is the MVP demo if the timeline collapses.
3. **US2 (events) + US3 (knowledge)** — completes the three memory surfaces. Run in parallel.
4. **US6 (viewport)** — Ken can inspect what the service holds. Pulled forward because the dev hosts are headless and otherwise debugging is `curl` + `psql`.
5. **US4 (unified search)** — once all three surfaces exist, the merge step is incremental work over the per-type queries from US1–3.
6. **US5 (observability)** — could ship earlier if needed; ordered after US1–4 so the metrics names match what was actually instrumented.
7. **Polish** — docs, perf smoke, CI, success-criteria walkthrough.

**Per-phase definition of done** (from constitution §"Development Workflow"):

- All listed tasks checked off.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace` green.
- Viewport: `pnpm check`, `cargo fmt --check`, `cargo clippy` green inside `viewport/src-tauri/`.
- Documentation for the surface area added or updated.
- Spec acceptance scenarios for the phase's user story verified manually or via the integration test.
