# klams setup

This document covers initial provisioning of the klams storage root
and runtime configuration. It is the manual counterpart to
[quickstart.md](../specs/001-initial-mvp/quickstart.md), pulled out
so it can be linked from the README.

## Storage root model

All klams persistent state lives under a single configurable directory
tree, by default `/ai/klams/`. See
[research.md §12](../specs/001-initial-mvp/research.md#12-storage-root)
for the rationale.

```text
$KLAMS_ROOT/                  (default /ai/klams)
├── config/
│   ├── klams.toml            # service config (incl. bearer token)
│   └── compose.env           # KLAMS_DATA_ROOT, image tags, secrets
├── data/                     # bind-mounted into containers
│   ├── postgres/
│   ├── qdrant/
│   └── tei/
└── logs/
```

Two environment variables are used:

| Variable          | Default              | Consumed by              |
|-------------------|----------------------|--------------------------|
| `KLAMS_ROOT`      | `/ai/klams`          | provision script, docs   |
| `KLAMS_DATA_ROOT` | `/ai/klams/data`     | `docker compose` (via `compose.env`) |

`KLAMS_DATA_ROOT` is derived from `KLAMS_ROOT` by the provision script
and written into `compose.env` so the compose file resolves to the
correct host paths.

## Provision the root

The repo ships `scripts/provision-storage-root.sh` which:

1. Creates `$KLAMS_ROOT/{config,data/postgres,data/qdrant,data/tei,logs}`.
2. Ensures the tree is owned by the invoking user.
3. Renders `$KLAMS_ROOT/config/klams.toml` and `compose.env` from the
   `deploy/` examples *only if absent* (idempotent).
4. Generates a fresh 32-byte hex bearer token and a Postgres password
   and injects them into both rendered files.

```bash
# default root (/ai/klams)
./scripts/provision-storage-root.sh

# dev host with a different root
KLAMS_ROOT=$HOME/.local/share/klams ./scripts/provision-storage-root.sh
```

After running, `$KLAMS_ROOT/config/klams.toml` and
`$KLAMS_ROOT/config/compose.env` are both `0600` and contain the same
generated secrets. Adjust them with your editor before bringing up
services.

## Overriding the root

For development on hosts other than `kubs0`, point everything at a
user-writable directory:

```bash
export KLAMS_ROOT=$HOME/.local/share/klams
./scripts/provision-storage-root.sh
docker compose --env-file $KLAMS_ROOT/config/compose.env \
  -f deploy/docker-compose.yml up -d postgres qdrant tei
KLAMS_CONFIG=$KLAMS_ROOT/config/klams.toml \
  cargo run --release -p klams-service
```

The compose file references `${KLAMS_DATA_ROOT}` for bind mounts, so
the data volumes follow the override automatically.

## Developer tooling

Sprint 002 introduces a top-level `justfile` so every routine
developer command (`gate`, `compose-up`, `run`, `verify`, …) is
discoverable via `just --list` and is the same command CI invokes.
Install [`just`](https://github.com/casey/just) once per developer
machine before running any Phase 2+ workflow (this is the install
referenced from sprint 002 task T001 and from
[specs/002-safety-and-write-ops/quickstart.md §Prerequisites](../specs/002-safety-and-write-ops/quickstart.md#prerequisites)).

```bash
# Debian / Ubuntu (and WSL Ubuntu): use the cargo install path so
# the binary lands on $PATH without a system package manager.
cargo install just

# Or, if you prefer the system package on a recent Debian/Ubuntu:
sudo apt-get install -y just

# Verify
just --version
```

After installing, `just --list` from the repo root prints every
recipe defined for this sprint. Run `just gate` before every
commit — it executes the constitution's pre-commit gate
(`cargo fmt --check`, `cargo clippy --workspace -- -D warnings`,
`cargo test --workspace`).

### `just --list` quick reference

| Recipe            | What it does |
|-------------------|--------------|
| `default`         | Prints this menu (`just --list`). |
| `compose-up`      | `docker compose -f deploy/docker-compose.yml up -d`. |
| `compose-down`    | Tears the stack down (keeps volumes). |
| `compose-rebuild` | `down` → `build --no-cache` → `up -d` for a clean image rebuild. |
| `build`           | `cargo build -p klams-service --release`. |
| `run`             | `cargo run -p klams-service`, logs to stderr. |
| `test`            | `cargo test --workspace` (skips `#[ignore]`'d cases). |
| `gate`            | Constitution pre-commit gate; what CI runs. |
| `health`          | `/healthz` curl + `scripts/verify-mvp.sh --light`. |
| `verify`          | Full `scripts/verify-mvp.sh` (SC-001..SC-009 smoke). |
| `viewport-build`  | `cargo xwin` Windows cross-build of the viewport. |

## Decay tuning (sprint 002)

The background decay task in `klams-service` re-weights facts on a
fixed interval so unused entries fall down the search ranking. The
defaults are baked into the binary; expose them by uncommenting the
`[decay]` block in `${KLAMS_ROOT}/config/klams.toml`:

```toml
[decay]
# How often the decay worker scans facts. Larger = less churn,
# slower convergence after a burst of writes.
task_interval_seconds = 3600   # default: 1h

# Facts updated per worker tick. Caps the DB transaction size so a
# huge corpus never holds a long-running write lock.
batch_size = 500               # default: 500

# Per-FactType decay rate λ in the formula new_w = old_w * exp(-λ · Δt)
# where Δt is seconds since last_used_at. Larger λ = decays faster.
# Defaults reflect Working memory (TaskFact) draining ~1000× faster
# than long-lived Machine/User facts.
[decay.lambda]
UserFact = 1e-9   # User memory — effectively permanent
TaskFact = 1e-6   # Working memory — fades within days of disuse
EnvFact  = 1e-9   # Machine memory — effectively permanent
```

The block is commented out in `deploy/config/klams.example.toml`;
uncomment only the overrides you actually want — any missing key
falls back to the baked-in default.
