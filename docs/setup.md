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
| `viewport-build-linux` | Native Linux build of the viewport (also works in WSL Ubuntu — see [usage.md](usage.md) for the runtime libs). |
| `viewport-run-linux`   | Build + launch the Linux viewport with `--debug`. |

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

## Sprint 003 — systemd deployment

After `cargo build --release` produces all three binaries, install
them with the helper:

```sh
just install-systemd          # builds + invokes deploy/install-systemd.sh
# or, to preview the actions without touching the system:
sudo deploy/install-systemd.sh --dry-run
```

The script is **idempotent** and:

1. Creates the `klams` system user (`useradd --system
   --no-create-home --shell /usr/sbin/nologin`) on first run.
2. Ensures `/var/lib/klams` and `/etc/klams` exist, owned by `klams`.
3. Stages `klams-service`, `klams-scanner`, `klams-monitor` into
   `/tmp/klams-stage-$$`, rotates any existing binary to
   `<bin>.prev`, then `mv -f`s the new one into `/usr/local/bin/`
   atomically. The `.prev` copy backs `just rollback`.
4. Installs `klams-service.service`, `klams-scanner.service`,
   `klams-scanner.timer`, and `klams-monitor.service` into
   `/etc/systemd/system/`.
5. `systemctl daemon-reload` then `enable --now` the service, timer,
   and monitor units.

Required preconditions (the script aborts loudly if any are
missing):

* `postgresql.service` is known to systemd on the host.
* Release binaries exist at `target/release/klams-{service,scanner,monitor}`.
* All four unit files exist under `deploy/`.

### Scanner config

`klams-scanner` reads `/etc/klams/scanner.toml` (override with
`KLAMS_CONFIG`):

```toml
url = "http://127.0.0.1:7777"
token = "<bearer from /etc/klams/klams.toml>"
roots = ["/home/ken/src", "/home/ken/obsidian"]
interval_secs = 3600            # ignored when running with --once
state_dir = "/var/lib/klams"    # SQLite cursor lives here
```

For ad-hoc runs (outside the timer): `just scanner-once`.

### Monitor config

`klams-monitor` reads `/etc/klams/monitor.toml`:

```toml
url = "http://127.0.0.1:7777"
token = "<bearer>"
poll_interval_secs = 30
services = [
    "klams-service.service",
    "postgresql.service",
    "docker.service",
]
```

The monitor only POSTs **edge transitions** — steady-state polls
generate zero traffic. For a one-off probe: `just monitor-once`.

### Logs

All three units log to the journal. Tail with:

```sh
journalctl -u klams-service -f
journalctl -u klams-scanner --since today
journalctl -u klams-monitor -f
```

### Rollback

`just rollback` swaps every `/usr/local/bin/<bin>` with its `.prev`
copy (created by `install-systemd.sh` on the previous upgrade) and
restarts the long-running units. No-op when no `.prev` exists.
