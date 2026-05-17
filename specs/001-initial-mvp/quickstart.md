# Quickstart: klams Initial MVP

This walkthrough takes a fresh checkout on a Linux dev box and ends
with the klams service running on `kubs0` and the viewport binary
ready to copy to a Windows workstation.

## Prerequisites

On the dev / build host (e.g. `kai` or `kubs0`):

- Rust (stable) via `rustup`; the workspace's `rust-toolchain.toml`
  pins MSRV.
- `cargo install cargo-xwin` for the viewport cross-build.
- `cargo install sqlx-cli --no-default-features --features postgres,native-tls`
  for offline migration prep.
- Docker Engine + Compose plugin (`docker compose` v2).
- `pnpm` (v9+) for the viewport frontend.
- NVIDIA Container Toolkit installed on `kubs0` (TEI uses the GPU).

## 1. Provision the storage root and runtime config

All stateful service data lives under a single configurable root
(default `/ai/klams`). This step is automated by
`/speckit.tasks` setup tasks; the manual equivalent is:

```bash
sudo mkdir -p /ai/klams/{config,data/postgres,data/qdrant,data/tei,logs}
sudo chown -R $USER:$USER /ai/klams

cp deploy/compose.env.example     /ai/klams/config/compose.env
cp deploy/config/klams.example.toml /ai/klams/config/klams.toml
$EDITOR /ai/klams/config/compose.env   # set bearer_token, image tags
$EDITOR /ai/klams/config/klams.toml    # match the token + URLs
```

`compose.env` exports `KLAMS_DATA_ROOT=/ai/klams/data` and any image
version pins.

## 2. Bring up dependencies

```bash
cd deploy/
docker compose --env-file /ai/klams/config/compose.env up -d \
  postgres qdrant tei
docker compose ps   # all three should be healthy on the klams-net network
```

This starts, all attached to the user-defined `klams-net` bridge
with DNS aliases `postgres`, `qdrant`, `tei`:

- `klams-postgres` on `127.0.0.1:5432`, data in `/ai/klams/data/postgres`
- `klams-qdrant` on `127.0.0.1:6333` (HTTP) / `127.0.0.1:6334` (gRPC),
  data in `/ai/klams/data/qdrant`
- `klams-tei` on `127.0.0.1:7070` with `BAAI/bge-small-en-v1.5`,
  model cache in `/ai/klams/data/tei`

Verify the shared network:

```bash
docker network inspect klams-net --format '{{range .Containers}}{{.Name}} {{end}}'
# expect: klams-postgres klams-qdrant klams-tei
```

## 3. Apply migrations and build the service

```bash
cd ..
# from repo root
export DATABASE_URL=postgres://klams:klams@127.0.0.1:5432/klams
sqlx migrate run --source migrations
cargo build --release -p klams-service
```

## 4. Run the service locally

```bash
KLAMS_CONFIG=/ai/klams/config/klams.toml \
  ./target/release/klams-service
```

Hit it:

```bash
TOKEN=$(grep bearer_token /ai/klams/config/klams.toml | cut -d'"' -f2)

curl -sS http://127.0.0.1:7777/healthz | jq

curl -sS -H "Authorization: Bearer $TOKEN" \
     -H 'content-type: application/json' \
     -d '{"type":"EnvFact","source":"User","payload":{"host":"kubs0","ram_gb":64}}' \
     http://127.0.0.1:7777/memory/facts | jq

curl -sS -H "Authorization: Bearer $TOKEN" \
     -H 'content-type: application/json' \
     -d '{"query":"kubs0","top_k":5}' \
     http://127.0.0.1:7777/memory/search | jq
```

## 5. Deploy on `kubs0`

```bash
sudo cp deploy/systemd/klams.service /etc/systemd/system/
sudo install -m 0755 target/release/klams-service /usr/local/bin/

# /etc/klams/klams.toml is a symlink to /ai/klams/config/klams.toml
sudo mkdir -p /etc/klams
sudo ln -sfn /ai/klams/config/klams.toml /etc/klams/klams.toml

sudo systemctl daemon-reload
sudo systemctl enable --now klams.service
sudo journalctl -u klams.service -f
```

The Compose stack from step 2 should already be running on `kubs0`
(start it via `docker compose --env-file /ai/klams/config/compose.env
up -d` from the repo's `deploy/` directory, or wrap it in a
`docker-compose@klams.service` systemd unit). All stack data lives
under `/ai/klams/data/` and every service is on the `klams-net`
network.

## 6. Build the viewport for Windows (from Linux)

```bash
cd viewport
pnpm install
pnpm build                          # SvelteKit static output → ./build
cd src-tauri
cargo xwin build --release --target x86_64-pc-windows-msvc
ls target/x86_64-pc-windows-msvc/release/klams-viewport.exe
```

Copy `klams-viewport.exe` to the Windows workstation, then create
`%APPDATA%\klams\viewport.toml`:

```toml
klams_url = "http://kubs0:7777"
refresh_interval_seconds = 10
```

Launch the viewport, paste the bearer token into the in-app config
dialog (stored via Windows Credential Manager), and the dashboard
should show a green connection indicator within a few seconds.

## 7. Run all tests

The test Compose file defines its own `klams-test-net` network and
uses ephemeral volumes (no bind mounts under `/ai/klams`) so it is
fully isolated from the production stack.

```bash
# from repo root
docker compose -f tests/docker-compose.test.yml up -d
DATABASE_URL=postgres://klams:klams@127.0.0.1:55432/klams \
  cargo test --workspace
docker compose -f tests/docker-compose.test.yml down -v
```

Viewport tests:

```bash
cd viewport
pnpm test            # vitest for lib/
cd src-tauri && cargo test
```

## 8. Pre-commit gate (constitution §"Pre-Commit Checks")

```bash
# Rust workspace (root)
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace

# Viewport
cd viewport
pnpm check           # svelte-check
pnpm lint            # eslint + prettier --check
cd src-tauri
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

All gates must pass clean before commit.

## 9. Smoke test the user stories

| Story | Smoke command |
|---|---|
| US1 — Fact write/read | `curl` upsert from §3, then a `/memory/search` with `types:["fact"]` and verify the fact appears. |
| US2 — Event append/query | `POST /memory/events` x3 with distinct `task_id`, then `GET /memory/events?task_id=…` and verify ordering. |
| US3 — Knowledge index/retrieve | `POST /memory/knowledge/index` with 3 chunks, then `POST /memory/search` with a paraphrase of one and verify it ranks first. |
| US4 — Unified search | Seed one of each type with a shared keyword, then `POST /memory/search` with no `types` filter; check all three appear. |
| US5 — Observability | `curl /healthz` (green); stop Qdrant; `curl /healthz` (degraded with reason); `curl /metrics` and grep for `klams_queue_depth`. |
| US6 — Viewport | Launch `klams-viewport.exe`; verify dashboard, browse each view, open one detail pane. |
