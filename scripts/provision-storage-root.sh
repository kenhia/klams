#!/usr/bin/env bash
# Provision the klams storage root and render runtime config.
#
# Idempotent: safe to re-run. Existing config files are left alone.
#
# Usage:
#   KLAMS_ROOT=/ai/klams ./scripts/provision-storage-root.sh
#
# Default KLAMS_ROOT is /ai/klams.

set -euo pipefail

KLAMS_ROOT="${KLAMS_ROOT:-/ai/klams}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXAMPLE_ENV="$REPO_ROOT/deploy/compose.env.example"
EXAMPLE_TOML="$REPO_ROOT/deploy/config/klams.example.toml"
EXAMPLE_SCANNER="$REPO_ROOT/deploy/config/scanner.example.toml"
EXAMPLE_MONITOR="$REPO_ROOT/deploy/config/monitor.example.toml"

if [[ ! -f "$EXAMPLE_ENV" || ! -f "$EXAMPLE_TOML" ]]; then
    echo "error: expected $EXAMPLE_ENV and $EXAMPLE_TOML to exist" >&2
    exit 1
fi
if [[ ! -f "$EXAMPLE_SCANNER" || ! -f "$EXAMPLE_MONITOR" ]]; then
    echo "error: expected $EXAMPLE_SCANNER and $EXAMPLE_MONITOR to exist" >&2
    exit 1
fi

echo "==> Provisioning klams storage root at: $KLAMS_ROOT"

mkdir -p \
    "$KLAMS_ROOT/config" \
    "$KLAMS_ROOT/data/postgres" \
    "$KLAMS_ROOT/data/qdrant" \
    "$KLAMS_ROOT/data/tei" \
    "$KLAMS_ROOT/logs"

if [[ "$(stat -c '%U' "$KLAMS_ROOT")" != "$USER" ]]; then
    echo "==> chown -R $USER:$USER $KLAMS_ROOT"
    chown -R "$USER:$USER" "$KLAMS_ROOT" 2>/dev/null || \
        sudo chown -R "$USER:$USER" "$KLAMS_ROOT"
fi

CONFIG_FILE="$KLAMS_ROOT/config/klams.toml"
ENV_FILE="$KLAMS_ROOT/config/compose.env"
SCANNER_FILE="$KLAMS_ROOT/config/scanner.toml"
MONITOR_FILE="$KLAMS_ROOT/config/monitor.toml"

generate_password() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 24
    else
        head -c 24 /dev/urandom | xxd -p -c 48
    fi
}

if [[ -f "$CONFIG_FILE" || -f "$ENV_FILE" ]]; then
    echo "==> Existing config detected; skipping render."
    echo "    klams.toml:   $CONFIG_FILE"
    echo "    compose.env:  $ENV_FILE"
    echo "    scanner.toml: $SCANNER_FILE"
    echo "    monitor.toml: $MONITOR_FILE"
else
    PG_PASSWORD="$(generate_password)"

    echo "==> Rendering $CONFIG_FILE"
    cp "$EXAMPLE_TOML" "$CONFIG_FILE"
    sed -i \
        -e "s|postgres://klams:changeme@127.0.0.1:5432/klams|postgres://klams:$PG_PASSWORD@127.0.0.1:5432/klams|" \
        "$CONFIG_FILE"
    # Sprint 034 (#773): the example ships every auth form commented out
    # (#670), so a rendered config MUST append a row or the service
    # refuses to start (AuthConfigError::NoTokens) — which is exactly what
    # this script silently produced between 032 and 034 (its old sed
    # pattern matched nothing in klams.example.toml).
    #
    # Sprint 050: those rows are `[[auth.identities]]` now. A klams
    # identity is a declared NAME, not a credential — nothing is
    # generated, nothing is printed, and nothing here needs protecting.
    # A caller sends `X-Homelab-Agent: <name>`; an unknown name is a 401.
    cat >>"$CONFIG_FILE" <<'TOML'

# Rendered by provision-storage-root.sh — the operator's starting
# identity (read+write+manage). Add rows per consumer as needed with
# `klams-token identity add`; the service reloads on SIGHUP. For admin
# verbs (restore / hard-delete), add an explicit admin scope — see
# docs/auth.md.
[[auth.identities]]
agent_name = "operator"
scopes     = ["read", "write", "manage"]
label      = "operator"

# Daemon identities, matching the rendered scanner.toml / monitor.toml.
# Deliberately without `manage` — they curate nothing.
[[auth.identities]]
agent_name = "klams-scanner"
scopes     = ["read", "write"]
label      = "scanner"

[[auth.identities]]
agent_name = "klams-monitor"
scopes     = ["read", "write"]
label      = "monitor"
TOML
    chmod 600 "$CONFIG_FILE"

    # Sprint 035 (#776): render scanner.toml and monitor.toml so the
    # operator is not left to copy the examples and discover the fields
    # the hard way. The scanner's \`roots\` placeholder is left as-is on
    # purpose — the scanner refuses to start until it points at real
    # paths.
    #
    # Sprint 050: nothing is substituted any more. The examples already
    # declare `agent = "klams-scanner"` / `"klams-monitor"`, matching the
    # identity rows appended above, and a name needs no rendering.
    echo "==> Rendering $SCANNER_FILE"
    cp "$EXAMPLE_SCANNER" "$SCANNER_FILE"
    chmod 600 "$SCANNER_FILE"

    echo "==> Rendering $MONITOR_FILE"
    cp "$EXAMPLE_MONITOR" "$MONITOR_FILE"
    chmod 600 "$MONITOR_FILE"

    echo "==> Rendering $ENV_FILE"
    cp "$EXAMPLE_ENV" "$ENV_FILE"
    # Force KLAMS_ROOT/KLAMS_DATA_ROOT to match this run, and inject the
    # generated password.
    sed -i \
        -e "s|^KLAMS_ROOT=.*|KLAMS_ROOT=$KLAMS_ROOT|" \
        -e "s|^KLAMS_DATA_ROOT=.*|KLAMS_DATA_ROOT=$KLAMS_ROOT/data|" \
        -e "s|^POSTGRES_PASSWORD=.*|POSTGRES_PASSWORD=$PG_PASSWORD|" \
        "$ENV_FILE"
    chmod 600 "$ENV_FILE"
fi

cat <<EOF

Done.
EOF
if [[ -n "${PG_PASSWORD:-}" ]]; then
    cat <<EOF

Your operator identity is "operator" (read+write+manage). klams has no
bearer tokens: callers declare a name and the service allow-lists it.

    curl -H "X-Homelab-Agent: operator" http://127.0.0.1:7777/memory/policy

EOF
fi
cat <<EOF
Next steps (docs/install.md walks these in full):

  1. Review and adjust (CPU-only hosts: see the decision tree in
     docs/install.md — TEI image tag, model, vector_dim all change):
       \$EDITOR $CONFIG_FILE
       \$EDITOR $ENV_FILE

  2. Bring up the backing services (postgres, qdrant, tei, reranker):
       cd $REPO_ROOT/deploy
       docker compose --env-file $ENV_FILE up -d

  3. Build and run klams-service:
       cd $REPO_ROOT
       cargo build --release -p klams-service
       KLAMS_CONFIG=$CONFIG_FILE ./target/release/klams-service

  4. Prove the install end-to-end (declares the operator identity):
       KLAMS_AGENT=operator just smoke

  5. To index your files, set \`roots\` in $SCANNER_FILE
     and run the scanner (its identity is already rendered).

EOF
