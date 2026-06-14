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

* `docker.service` is known to systemd on the host — Postgres, Qdrant,
  and the TEI embedding server run in Docker (the units declare
  `After=/Wants=docker.service`). There is no host `postgresql.service`.
* Release binaries exist at `target/release/klams-{service,scanner,monitor}`.
* All four unit files exist under `deploy/`.

### Scanner config

`klams-scanner` reads `/etc/klams/scanner.toml` (override with
`KLAMS_CONFIG`):

```toml
url = "http://127.0.0.1:7777"
token = "<bearer from /etc/klams/klams.toml>"
roots = ["/home/ken/src", "/home/ken/obsidian"]   # MUST be absolute
interval_secs = 3600            # ignored when running with --once
state_dir = "/var/lib/klams"    # SQLite cursor lives here
```

`roots` **must be absolute paths**. The walker honours `.gitignore`
and a repo-root `.klamsignore`, and always prunes heavy dependency /
cache trees (`target`, `node_modules`, `.pnpm-store`, `.venv`,
`__pycache__`, `.obsidian`, `dist`, `build`, …) *before* descent.

**Read access:** the scanner runs as the `klams` system user, so every
path under `roots` must be readable by `klams`. For files owned by
another user (e.g. `ken`), grant a default ACL so newly-created files
stay readable, e.g.:

```sh
sudo setfacl -R  -m u:klams:rX /home/ken/src /home/ken/obsidian
sudo setfacl -R -d -m u:klams:rX /home/ken/src /home/ken/obsidian   # inherit
```

For ad-hoc runs (outside the timer): `just scanner-once`.

### Monitor config

`klams-monitor` reads `/etc/klams/monitor.toml`:

```toml
url = "http://127.0.0.1:7777"
token = "<bearer>"
units = ["klams-service.service"]   # systemd units to watch
interval_secs = 30
# host = "kubs0"                     # optional; defaults to $HOSTNAME
```

The config keys are `units` (the systemd units to poll) and
`interval_secs` (poll cadence). The monitor only POSTs **edge
transitions** (Up / Down / VersionChanged) — steady-state polls
generate zero traffic. For a one-off probe: `just monitor-once`.

### Config file permissions

`/etc/klams/klams.toml`, `scanner.toml`, and `monitor.toml` hold bearer
tokens and the Postgres DSN. They MUST be `root:klams 0640` (owner
root, group `klams`, group-readable) so the `klams`-user daemons can
read them while keeping the secrets off world-read. A wrong owner/mode
(e.g. `root:root 0600`) makes a daemon crash-loop with
`read config …: Permission denied (os error 13)`.

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

## Sprint 006 — Restore from snapshot

The nightly backup task lands `postgres-<UTC-date>.dump` and
`qdrant-<UTC-date>.snapshot` pairs into `[backup].backup_dir`
(see [usage.md](usage.md#sprint-006--maintenance-window--backups) for
the `[backup]` config block). Restoring from one of those pairs is
the once-exercised DR drill that satisfies **FR-016** and is
walked end-to-end in
[specs/006-maintenance-and-backups/quickstart.md §5](../specs/006-maintenance-and-backups/quickstart.md#5-restore-from-a-snapshot-fr-016).
See [specs/006-maintenance-and-backups/spec.md](../specs/006-maintenance-and-backups/spec.md)
for the requirements this procedure satisfies.

```bash
# 1. Record current state for comparison
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM facts;"  -t > /tmp/pre-counts-facts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM events;" -t > /tmp/pre-counts-events

# 2. Tear down the live stack (loses all in-memory state)
docker compose -f tests/docker-compose.test.yml down -v

# 3. Bring up a fresh stack
docker compose -f tests/docker-compose.test.yml up -d
just wait-for-stack

# 4. Restore from yesterday's snapshot
just restore-from $(date -u -d 'yesterday' +%F)

# 5. Compare counts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM facts;"  -t > /tmp/post-counts-facts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM events;" -t > /tmp/post-counts-events
diff /tmp/pre-counts-facts  /tmp/post-counts-facts  && echo "facts match"
diff /tmp/pre-counts-events /tmp/post-counts-events && echo "events match"
```

> **`--force` is required against a non-empty target.** `just
> restore-from <date>` refuses to overwrite a Postgres that already
> has rows in `facts` / `events` / `knowledge_items` and exits
> non-zero with `target is non-empty; pass --force to overwrite`.
> Pass `--force` only when you have already accepted the data loss:
>
> ```bash
> just restore-from 2026-05-22            # fails: non-empty target
> just restore-from 2026-05-22 --force    # succeeds: drops + reloads
> ```

`pg_restore` is invoked with `--single-transaction --clean
--if-exists`, so a failed mid-restore rolls back atomically and the
target Postgres is left in its pre-call state.

## Sprint 007 — MCP server registration

Sprint 007 mounts a Model Context Protocol surface on
`klams-service`. Once tokens are configured (see
[usage.md](usage.md#sprint-007--mcp-server) for the `[[auth.tokens]]`
shape) and the service is running, register klams with each MCP
client you want to wire in.

Step-by-step walkthrough lives at
[specs/007-mcp-server/quickstart.md](../specs/007-mcp-server/quickstart.md).

### VS Code (`.vscode/mcp.json`)

Create or extend `<workspace>/.vscode/mcp.json`:

```jsonc
{
  "servers": {
    "klams": {
      "type": "http",
      "url": "http://kubs0:7777/mcp",
      "headers": {
        "Authorization": "Bearer ghcp-write-XXXXXXXXXXXXXXXX"
      }
    }
  }
}
```

Reload the VS Code window. The status bar shows the klams MCP server
connected and GHCP's tool palette lists the klams tools (filtered
to the scopes the token grants — `read` + `write` for `ghcp`).

VS Code's "MCP: klams" Output panel will log two harmless warnings
on startup (`Could not fetch resource metadata` and
`Failed to parse message: ""`) — see
[specs/007-mcp-server/research-vscode-mcp-http.md](../specs/007-mcp-server/research-vscode-mcp-http.md)
§6–§7 for what they mean. They can be ignored.

If you connect via a non-loopback hostname and want belt-and-suspenders
DNS-rebinding protection, set `[server].mcp_allowed_hosts` in
`klams.toml` (default empty = disabled; bearer auth is the real gate).

### GHCP CLI (`~/.copilot/mcp-config.json`)

```jsonc
{
  "mcpServers": {
    "klams": {
      "type": "http",
      "url": "http://kubs0:7777/mcp",
      "headers": {
        "Authorization": "Bearer ghcp-write-XXXXXXXXXXXXXXXX"
      },
      "tools": ["*"]
    }
  }
}
```

Restart the GHCP CLI and verify:

```bash
copilot mcp tools klams
```

Expected: `register_author`, `memory_add`, `memory_search`,
`memory_related`, `memory_delete`, `memory_append_event`. The
`memory_admin_*` tools are absent unless the bearer token also
carries `admin` scope (FR-020).

### Per-token scope tips

- One read-only token for the viewport so a UI compromise cannot
  mutate state.
- One read+write token per agent that produces memories (typically
  one per editor).
- One admin token, used only from your own shell, for restores and
  hard deletes.

## Sprint 008 — Observability profile (Prometheus + Grafana)

Sprint 008 lights up the per-author MCP activity panels added to
[`deploy/grafana/klams.json`](../deploy/grafana/klams.json). The
underlying counters (`klams_mcp_writes_total`,
`klams_mcp_deletes_total`, `klams_mcp_search_total`) have been
emitted since sprint 007 but had no dashboard rendering them. The
`observability` Compose profile is opt-in; the production stack on
`kubs0` continues to run without Prometheus or Grafana unless you
ask for them.

### Bring the observability profile up

```bash
docker compose --profile observability up -d prometheus grafana
```

This starts two extra containers:

- `klams-prometheus` (image pinned via `PROMETHEUS_IMAGE_TAG` in
  `compose.env`, default `v2.55.0`), bound to `127.0.0.1:9090`.
  Scrape config is mounted from
  [`deploy/prometheus/prometheus.yml`](../deploy/prometheus/prometheus.yml);
  WAL/TSDB lives under `${KLAMS_DATA_ROOT}/prometheus`. The single
  scrape job targets `klams-service:7777/metrics` with the
  service bearer token.
- `klams-grafana` (image pinned via `GRAFANA_IMAGE_TAG`, default
  `11.2.2`), bound to `127.0.0.1:3000`. The klams dashboard JSON is
  bind-mounted read-only at `/var/lib/grafana/dashboards/klams.json`
  and provisioned via the Compose-managed `dashboards.yaml` so
  edits are reverted on container restart — the JSON is the source
  of truth.

### Reload the dashboard after editing the JSON

Grafana reloads provisioned dashboards on file change, but a
container restart is the deterministic way:

```bash
docker compose restart klams-grafana
```

Open `http://127.0.0.1:3000` (default admin/admin on first boot,
prompt to change), browse to **Dashboards → klams**, and confirm the
three **MCP author activity** panels render. The panel-vs-series
contract test
[`crates/klams-service/tests/grafana_dashboard_json.rs`](../crates/klams-service/tests/grafana_dashboard_json.rs)
keeps the dashboard JSON in lock-step with the
[ansible-k klams-grafana.md handoff doc](https://github.com/kenhia/ansible-k/blob/main/specs/klams-integration/klams-grafana.md);
every series referenced by a panel must appear in that handoff
table or the workspace gate fails.

### Tear the profile down

```bash
docker compose --profile observability down
```

This stops Prometheus and Grafana only; the postgres/qdrant/tei
services keep running. The on-disk volumes under
`${KLAMS_DATA_ROOT}/prometheus` and `${KLAMS_DATA_ROOT}/grafana`
persist across restarts.

## Sprint 009 — Stability & attribution

Sprint 009 (`specs/009-stability-attribution/`) introduces three
operator-visible configuration knobs: the raised file-descriptor cap
in the systemd unit, the `agent_name` field on bearer tokens, and the
one-shot re-attribution CLI.

### Raised file-descriptor cap (`LimitNOFILE=65536`)

The shipped [`deploy/klams-service.service`](../deploy/klams-service.service)
unit sets `LimitNOFILE=65536` so the connection-limits layer can
reach its per-peer budget before the kernel-level fd cap is hit
(FR-005, SC-002). After deploying a new copy of the unit:

```bash
sudo install -m 0644 deploy/klams-service.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl restart klams-service
cat /proc/$(pidof klams-service)/limits | grep -i 'open files'
# Expect: Max open files            65536    65536    files
```

If the running process shows a lower cap (e.g. the host default of
1024), `systemctl daemon-reload` was missed, or another drop-in unit
is overriding the value. Check with
`systemctl cat klams-service.service`.

> **Note for hosts where postgres/qdrant run under Docker** (the
> `kubs0` topology): the upstream unit hard-required
> `postgresql.service` and `qdrant.service`. If you manage those
> dependencies via Compose instead of native systemd units, drop the
> `Requires=` line and switch `After=` to `docker.service network-online.target`.
> Otherwise systemctl will refuse to start the service citing
> missing dependencies.

### Token attribution (`agent_name`)

Each `[[auth.tokens]]` entry in `klams.toml` now accepts an optional
`agent_name` field. The agent name is resolved to an `author_id` at
service startup (the row is created in the `authors` table if it
doesn't already exist) and cached for the life of the process.
Every REST write under that bearer is then attributed to the
resolved author.

```toml
[[auth.tokens]]
token = "ghcp-write-XXXXXXXXXXXXXXXX"
scopes = ["read", "write"]
agent_name = "ghcp"            # ← NEW; lowercase, digits, '-' or '_'

[[auth.tokens]]
token = "viewport-read-XXXXXXXXXXXXXXXX"
scopes = ["read"]
agent_name = "viewport"

[[auth.tokens]]
token = "bench-XXXXXXXXXXXXXXXX"
scopes = ["read", "write"]
agent_name = "klams-bench"      # required for author-based bench-clean
```

Rules (enforced at startup; the service refuses to start on
violation):

- Charset: `[a-z0-9_-]+`. Uppercase, dots, spaces, and other
  punctuation are rejected with a clear error before the listener
  binds.
- Length: 1–128 characters.
- Multiple tokens may share an `agent_name` — they all resolve to
  the same `author_id`. Useful for rotating tokens without losing
  attribution continuity.
- Tokens without `agent_name` fall back to the seeded `system`
  author so existing deployments keep working unchanged.
- The legacy single `[auth].bearer_token` field is always
  materialized as a `system`-bound grant.

### One-shot re-attribution repair

Deployments with historical REST writes from before sprint 009
shipped will have those rows stamped as `system`. The standalone
[`tools/reattribute-system/`](../tools/reattribute-system/) CLI
walks the affected rows and reassigns each to the author of the
`register_author` event that immediately preceded the write. Rows
with no resolvable antecedent land on the new seeded `lost-author`
identity rather than staying on `system`, preserving the per-author
bucket sum (FR-016, FR-016a).

```bash
# Dry-run; prints the per-author reassignment plan and the
# `lost-author` bucket count without touching the store.
cargo run --release -p reattribute-system

# Commit:
cargo run --release -p reattribute-system -- --apply
```

The repair is **idempotent** — a second `--apply` is a no-op once
every row has been classified. Run once as part of the cutover; no
recurring schedule is needed.
