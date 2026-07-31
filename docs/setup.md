# klams setup

This document covers initial provisioning of the klams storage root
and runtime configuration. It is the manual counterpart to
[quickstart.md](../sprints/001-initial-mvp/quickstart.md), pulled out
so it can be linked from the README.

## Storage root model

All klams persistent state lives under a single configurable directory
tree, by default `/ai/klams/`. See
[research.md §12](../sprints/001-initial-mvp/research.md#12-storage-root)
for the rationale.

```text
$KLAMS_ROOT/                  (default /ai/klams)
├── config/
│   ├── klams.toml            # service config (incl. [[auth.tokens]] grants)
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
4. Generates a fresh Postgres password (injected into both rendered
   files) and a 32-byte hex operator token, appended to the rendered
   `klams.toml` as a scoped `[[auth.tokens]]` grant —
   `scopes = ["read", "write", "manage"]`, `label = "operator"`,
   `agent_name = "operator"` — and printed once at the end of the run.
   The printed next steps close with a
   `curl -H "Authorization: Bearer <token>" /healthz` round-trip, so a
   fresh provision is verified working rather than assumed.

Sprint 034 (#773) fixed step 4: between sprints 032 and 034 the script
sed'd a token placeholder that no longer existed in
`klams.example.toml` (every token form there ships commented out since
#670), so the rendered config had **no** active grant and the service
refused to start (`AuthConfigError::NoTokens`).

```bash
# default root (/ai/klams)
./scripts/provision-storage-root.sh

# dev host with a different root
KLAMS_ROOT=$HOME/.local/share/klams ./scripts/provision-storage-root.sh
```

After running, `$KLAMS_ROOT/config/klams.toml` and
`$KLAMS_ROOT/config/compose.env` are both `0600`; the Postgres
password lands in both, the operator token only in `klams.toml` (and
once on stdout). Adjust them with your editor before bringing up
services.

## Overriding the root

For development on hosts other than `kubs0`, point everything at a
user-writable directory:

```bash
export KLAMS_ROOT=$HOME/.local/share/klams
./scripts/provision-storage-root.sh
docker compose --env-file $KLAMS_ROOT/config/compose.env \
  -f deploy/docker-compose.yml up -d postgres qdrant tei reranker
KLAMS_CONFIG=$KLAMS_ROOT/config/klams.toml \
  cargo run --release -p klams-service
```

The compose file references `${KLAMS_DATA_ROOT}` for bind mounts, so
the data volumes follow the override automatically.

### How `klams-service` finds its config (sprint 034, #775)

`klams-service` resolves its config path in order:

1. `$KLAMS_CONFIG`, when set — always wins. The shipped systemd unit
   sets it to `/etc/klams/klams.toml`, so hardened installs are
   unaffected by the fallbacks below.
2. `/ai/klams/config/klams.toml`, when that file exists — the
   storage-root default the provision script renders.
3. `$XDG_CONFIG_HOME/klams/klams.toml` (default
   `~/.config/klams/klams.toml`; an empty `XDG_CONFIG_HOME` counts as
   unset, per the XDG spec).

If none exists the startup error names both tried paths. Before
sprint 034 the docs treated `/ai/klams/config/klams.toml` as the one
true default, which left dev hosts without a storage root exporting
`KLAMS_CONFIG` by hand for every command. The `justfile` mirrors the
same rule via its `klams_config` variable, so `just run`,
`just backup-validate-config`, and the service agree on which file
they read.

## The reranker service (sprint 030)

`reranker` is a second TEI container (same image tag as `tei`, model
`${RERANKER_MODEL_ID}` — `BAAI/bge-reranker-v2-m3`) serving the
optional `memory_search` cross-encoder stage on `127.0.0.1:7071`. On
kubs0 include the GPU override (`deploy/docker-compose.gpu.yml`) so
both TEI containers share the 4080 SUPER via CDI (~2.9 GB VRAM
total). The stage activates only when `/etc/klams/klams.toml` sets
`[retrieval] reranker_url`; removing that key (or stopping the
container — the stage is best-effort) is the rollback. The model id
is NOT the Qwen3-Reranker the planning WI named: TEI cannot serve
that architecture yet (upstream PRs open, 2026-07-26) — swap
`RERANKER_MODEL_ID` when a TEI release merges support.

Since sprint 036 the configured reranker serves **both** search
surfaces (REST `/memory/search` runs the same core pipeline as MCP
`memory_search`, #730) and is visible on `/healthz` as a non-fatal
`reranker` subsystem (#731). The **test stack**
(`tests/docker-compose.test.yml`) carries its own CPU reranker
(`BAAI/bge-reranker-base`, port `127.0.0.1:57071`) so the live-rerank
integration tests run under `just test-integration` — smaller than
production for the same reason the test embedder is bge-small, and
wired via `TEST_RERANKER_URL` by the recipe.

## Developer tooling

Sprint 002 introduces a top-level `justfile` so every routine
developer command (`gate`, `compose-up`, `run`, `verify`, …) is
discoverable via `just --list` and is the same command CI invokes.
Install [`just`](https://github.com/casey/just) once per developer
machine before running any Phase 2+ workflow (this is the install
referenced from sprint 002 task T001 and from
[sprints/002-safety-and-write-ops/quickstart.md §Prerequisites](../sprints/002-safety-and-write-ops/quickstart.md#prerequisites)).

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

# Per-FactType decay rate λ in the formula w = 1 / (1 + λ · age),
# where age is seconds since last_used_at. Hyperbolic, recomputed from
# total age each sweep — not exponential and not compounded (#648
# corrected this; half-life is 1/λ seconds, so 1e-6 = ~11.6 days).
# Larger λ = decays faster.
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

### Enabling `[backup]` needs a `ReadWritePaths=` drop-in (sprint 034, #774)

`klams-service.service` runs under `ProtectSystem=strict`, so every
writable path outside `StateDirectory` needs an explicit
`ReadWritePaths=` grant — and since sprint 034 (#774) the shipped unit
carries **none**. It used to hardcode `ReadWritePaths=/gratch/klams-backup`,
and systemd refuses to start a unit whose `ReadWritePaths` target is
missing on the host, so the shipped unit only started on kubs0. Hosts
that enable `[backup]` add the grant back as a drop-in instead of
editing the unit (kubs0 already carries this drop-in, pointing at
`/gratch/klams-backup`):

```ini
# /etc/systemd/system/klams-service.service.d/backup.conf
[Service]
ReadWritePaths=/path/to/backup_dir
```

Then `systemctl daemon-reload && systemctl restart klams-service`. If
`backup_dir` is a network mount, add
`RequiresMountsFor=/path/to/backup_dir` in the same drop-in. Without
the grant, every nightly backup dies on the lockfile with `EROFS` —
the sprint 020 failure mode.

### Upgrading an already-running install: `just restart` is required

`enable --now` in step 5 is a **no-op for a unit that is already
running**, so on an upgrade the new binary lands in `/usr/local/bin`
while the old process keeps serving. Follow the install with:

```sh
just restart                  # klams-service + klams-monitor
```

Then confirm the version actually moved — this is what the
sprint-numbered PATCH version on `/healthz` is for:

```sh
curl -s http://127.0.0.1:7777/healthz | jq .version
```

If it still reports the previous version, the restart did not take.
The scanner needs nothing: it is timer-driven and picks up the new
binary on its next fire.

A restart also **applies pending migrations** — `PostgresStore::connect`
runs them as a side effect. `just rollback` swaps binaries only and
cannot undo one; crossing a migration boundary backwards needs
`just restore-from <date>`.

**Sprint 027 adds `0012_oversize_write.sql`** (additive: one new table,
no changes to existing ones) and two config keys under `[embeddings]`:

| Key | Default | What it does |
|-----|---------|--------------|
| `max_input_tokens` | `512` | The embedding model's input ceiling. Every ingest path gates against it. Verify with `curl -s http://127.0.0.1:7070/info \| jq .max_input_length` before changing. |
| `oversize_log_retention_days` | `90` | How long refused-write rows (which hold full payloads) are kept before the daily prune. |

**The scanner has a matching `max_input_tokens` in `/etc/klams/scanner.toml`
and the two must be kept in step.** They are separate processes on
separate hosts, so nothing enforces agreement: if the scanner's value is
higher than the service's, it will publish chunks the service refuses,
and those files' cursors will stall (visibly, in the scanner's logs —
not silently, which is the pre-027 behaviour this replaces).

The whole procedure, with preflight and verification, is packaged as
the repo-local `deploy-kubs0` skill (`.claude/skills/deploy-kubs0/`),
which `/sprint-ship` invokes automatically via `.sprint-deploy`.

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
roots = ["/home/ken/src"]       # MUST be absolute
interval_secs = 3600            # ignored when running with --once
state_dir = "/var/lib/klams"    # SQLite cursor lives here
```

> **The Obsidian vault is deliberately NOT scanned** (sprint 028,
> WI #657 — Ken's decision, 2026-07-25). The vault is largely
> historical notes; to a recall-first agent a confident stale hit costs
> more than a miss. Do not reinstate `/home/ken/obsidian` on a rebuild.
> If specific vault subtrees earn their way back in, add them as
> targeted roots behind a `.klamsignore` allowlist — not the whole
> vault.

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
# host = "kubs0"                     # optional; defaults to the system hostname (/proc/sys/kernel/hostname)
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
[sprints/006-maintenance-and-backups/quickstart.md §5](../sprints/006-maintenance-and-backups/quickstart.md#5-restore-from-a-snapshot-fr-016).
See [sprints/006-maintenance-and-backups/spec.md](../sprints/006-maintenance-and-backups/spec.md)
for the requirements this procedure satisfies.

```bash
# 1. Record current state for comparison
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM facts;"  -t > /tmp/pre-counts-facts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM events;" -t > /tmp/pre-counts-events

# 2. Tear down the live stack (loses all in-memory state)
docker compose -f tests/docker-compose.test.yml down -v

# 3. Bring up a fresh stack
docker compose -f tests/docker-compose.test.yml up -d
# `just wait-for-stack` was cited here until sprint 032 (#648); no such
# recipe has ever existed. Poll the containers instead:
until [ "$(docker compose -f tests/docker-compose.test.yml ps \
        --format '{{.Health}}' | sort -u)" = "healthy" ]; do sleep 2; done

# 4. Restore from yesterday's snapshot
just restore-from $(date -u -d 'yesterday' +%F)

# 5. Compare counts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM facts;"  -t > /tmp/post-counts-facts
psql "$DATABASE_URL" -c "SELECT COUNT(*) FROM events;" -t > /tmp/post-counts-events
diff /tmp/pre-counts-facts  /tmp/post-counts-facts  && echo "facts match"
diff /tmp/pre-counts-events /tmp/post-counts-events && echo "events match"
```

> **`--force` is required against a non-empty target.** `just
> restore-from <date>` refuses to overwrite a non-empty target and
> exits non-zero with `target is non-empty; pass --force to
> overwrite`. The guard probes the two stores separately
> (`crates/klams-service/src/backup/restore.rs`): Postgres rows in
> `facts` / `events`, and Qdrant points in the knowledge collection —
> knowledge lives only in Qdrant; Postgres has no knowledge table.
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
[sprints/007-mcp-server/quickstart.md](../sprints/007-mcp-server/quickstart.md).

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
[sprints/007-mcp-server/research-vscode-mcp-http.md](../sprints/007-mcp-server/research-vscode-mcp-http.md)
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

Scopes are **flat** — `write` does not imply `read`, `admin` does not
imply `write`. List every scope a token needs. Full model:
[auth.md](auth.md).

- **A UI token needs only `["read"]`.**
  [klams-view](https://github.com/kenhia/klams-view), the dashboard,
  reads and nothing else. (This page has said the opposite before: the
  retired `viewport` app was a *curation* surface and genuinely needed
  `manage`. Scope the token to what the client does — see the note in
  [auth.md](auth.md#recommended-token-split) for the full history.)
- **Curation has no UI right now.** Resolving dissents is a `manage`
  scoped REST call; keep an operator token for it. See
  [auth.md](auth.md#resolving-dissents-without-a-ui).
- One read+write token per agent that produces memories (typically
  one per editor). Add `manage` only for agents you want curating other
  authors' records.
- One admin token, used only from your own shell, for restores and
  hard deletes.
- Give every token an `agent_name`. Ownership on `memory_delete` is
  decided by the bound author, so a token without one cannot delete.

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
keeps the dashboard JSON in lock-step with the series contract at
[`deploy/grafana/SERIES.md`](../deploy/grafana/SERIES.md). Sprint 032
(#680) moved that contract here from the inert ansible-k repo, where
the test read it from a sibling checkout and self-skipped when absent —
so it was a silent no-op on CI. It now checks both directions
unconditionally: a panel may not query an undocumented series, and code
may not declare one.

### Tear the profile down

```bash
docker compose --profile observability down
```

This stops Prometheus and Grafana only; the postgres/qdrant/tei
services keep running. The on-disk volumes under
`${KLAMS_DATA_ROOT}/prometheus` and `${KLAMS_DATA_ROOT}/grafana`
persist across restarts.

### Production scrape path (kubsdb Prometheus + Grafana)

The bundled `observability` profile above is for a self-contained
dev/test stack. In the live deployment the dashboard is rendered by a
**central** Prometheus + Grafana running on `kubsdb`, not by the
compose profile:

- **Prometheus** (`kubsdb:9090`) scrapes the service over the network
  via a `klams-service` job whose target is `kubs0:7777`
  (`metrics_path: /metrics`, label `service: klams-service`). The
  Prometheus container resolves `kubs0` through an `extra_hosts`
  entry. `--web.enable-lifecycle` is on, so config changes hot-reload
  with `curl -X POST http://kubsdb:9090/-/reload` (no restart). The
  infra-as-code source is the ansible-k repo
  (`roles/prometheus/templates/prometheus.yml.j2`).
- **Grafana** (`kubsdb:3000`) holds the klams dashboard (uid
  `klams-006`). Panels pin the datasource by uid `prometheus`. The
  repo copy [`deploy/grafana/klams.json`](../deploy/grafana/klams.json)
  is the source of truth; push updates with the Grafana HTTP API
  (`POST /api/dashboards/db`, `overwrite: true`).

The service exposes the metric names the dashboard expects:
`klams_queue_depth`, `klams_workers_active`,
`klams_summarization_lag_seconds`, the `klams_mcp_*` counters, and the
HTTP/latency series under the `axum_http_requests_total` /
`axum_http_requests_duration_seconds_bucket` families (labelled by
`endpoint`). The backup/maintenance panels
(`klams_backup_*`, `klams_maintenance_mode_active`) are emitted
**lazily** by the backup hook subsystem — they appear only after the
first backup/maintenance event since service start, so those panels
read "No Data" on a freshly restarted service until a backup runs.

## Sprint 009 — Stability & attribution

Sprint 009 (`sprints/009-stability-attribution/`) introduces three
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

Each `[[auth.tokens]]` entry in `klams.toml` accepts an `agent_name`
field (optional for `read`/`write`-only grants; mandatory when the
grant holds `manage` or `admin` — sprint 034, #703, see the rules
below). The agent name is resolved to an `author_id` at
service startup (the row is created in the `authors` table if it
doesn't already exist) and again on each [auth reload](#hot-reloading-authtokens).
Every REST write under that bearer is then attributed to the
resolved author, and MCP write tools (`memory_add`,
`memory_append_event`, `dissent_propose`) fall back to it when the
caller omits `author_id` (sprint 018, WI #62).

Since sprint 025 the binding is also an **authorization** input, not
just an attribution one: `memory_delete` acts as the bound author and
refuses to delete another author's memory unless the token carries the
`manage` scope. A token with no `agent_name` cannot delete at all. See
[auth.md](auth.md) for the full model.

```toml
[[auth.tokens]]
token = "ghcp-write-XXXXXXXXXXXXXXXX"
scopes = ["read", "write"]
agent_name = "ghcp"            # ← NEW; lowercase, digits, '-' or '_'

[[auth.tokens]]
token = "klams-view-XXXXXXXXXXXXXXXX"
scopes = ["read"]                      # the dashboard only reads
agent_name = "klams-view"

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
- Tokens without `agent_name` fall back to the seeded `system` author
  — but only for unprivileged grants: since sprint 034 (#703) a grant
  holding `manage` or `admin` must declare `agent_name`, so privileged
  actions are attributable.
- The legacy single `[auth].bearer_token` field is **retired**
  (sprint 034, #703). It used to materialize as a `system`-bound
  all-scope grant — exactly the unattributable privileged credential
  the previous rule forbids. The key still parses so it can be
  refused loudly: a config that sets it fails startup,
  `--validate-config`, and SIGHUP reload alike. Migration note:
  [auth.md](auth.md). At least one `[[auth.tokens]]` grant is now
  required.

### Hot-reloading `[[auth.tokens]]`

Sprint 018 (WI #61): adding, removing, or rotating bearer tokens no
longer needs a service restart. Send SIGHUP and the service re-reads
`klams.toml`, re-resolves token→author bindings, and atomically swaps
the in-memory token table shared by the REST and `/mcp` surfaces:

```bash
sudo systemctl reload klams-service      # unit ships ExecReload=kill -HUP
# or, without systemd:
kill -HUP "$(pidof klams-service)"
```

Semantics:

- New `[[auth.tokens]]` entries authenticate immediately after the
  reload; removed entries stop authenticating. In-flight requests are
  not dropped — a request already past its auth check completes
  normally.
- Only the `[auth]` block is applied. Changes to any other section
  (postgres, qdrant, embeddings, backup, …) still require a restart.
- A reload that fails (unparseable TOML, no `[[auth.tokens]]` grants,
  a still-set retired `bearer_token`, invalid or missing
  `agent_name`) is logged as an error and the **previous token table
  stays active** — a broken edit can't lock every caller out. Check
  `journalctl -u klams-service -g SIGHUP` for the outcome; run
  `klams-service --validate-config` before reloading to catch errors
  up front.
- The file permission model is unchanged: the reload reads the same
  `root:klams 0640` file.

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
cargo run --release -p reattribute-system -- --dry-run

# Commit:
cargo run --release -p reattribute-system -- --apply
```

Exactly one of `--dry-run` / `--apply` is **required** — running it
bare exits 2, so the first form above needed the flag it was missing
(#648). It targets `knowledge_items_v2` by default and refuses to run
against a collection that does not exist; override with
`KLAMS_QDRANT_COLLECTION` if the live `collection` in
`/etc/klams/klams.toml` ever differs. Before sprint 032 its default
named a collection that had never existed in production, and because
`QdrantStore::connect` creates on absence, a bare run manufactured that
collection and reported zero repairs (#647).

The repair is **idempotent** — a second `--apply` is a no-op once
every row has been classified. Run once as part of the cutover; no
recurring schedule is needed.

## Sprint 023 — multi-host scanning

The scanner is a client that POSTs to the central klams service, so
scanning a second host is just running it there pointed at kubs0. Every
chunk now carries its **host** (`machine`), keyed into the delete +
dedupe path, so two hosts that share a path (`/home/ken/src/...` on both)
never corrupt each other, and `memory_search` knowledge results include
the host for a fully-qualified `(host, source_path)`.

### Deploying a scanner on a second host (e.g. kai)

1. **Token:** add a dedicated `[[auth.tokens]]` to `/etc/klams/klams.toml`
   on kubs0 (write scope, `agent_name = "kai-scanner"`), then
   `sudo systemctl reload klams-service` (hot-reload, no restart — sprint
   018). Keep it distinct from kubs0's own scanner token so writes stay
   attributable per host.
2. **Binary:** install the same-version `klams-scanner` on kai (it's the
   same Linux release binary; version must match the service it writes
   to).
3. **Config:** `/etc/klams/scanner.toml` with `url = "http://kubs0:7777"`,
   the `kai-scanner` token, and kai's absolute roots (at least
   `/home/ken/src`). Leave `host` unset — the scanner reports kai's
   kernel hostname automatically.
4. **Unit:** install `klams-scanner.service` + `.timer`, but **drop the
   `After=klams-service.service`** dependency (there is no local service
   on kai — it depends only on `network-online.target`). Then
   `systemctl enable --now klams-scanner.timer`.

Verify: after a scan, a kai-only file is retrievable via `memory_search`
with `host = "kai"`, and kubs0 files show `host = "kubs0"`.

**Failure mode (why per-host, not central mount):** if kai is down, its
scanner simply doesn't run — kai's chunks go *stale*, never deleted. A
central mount-and-scan topology (for hosts that can't run the scanner,
e.g. Windows/cleo) is a separate future option (klams #406) with a
`NOT_MOUNTED` sentinel guard against a mount outage triggering a mass
prune.
