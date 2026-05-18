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
