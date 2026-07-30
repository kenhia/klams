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

TOKEN_FILE="$KLAMS_ROOT/config/klams.toml"
ENV_FILE="$KLAMS_ROOT/config/compose.env"
SCANNER_FILE="$KLAMS_ROOT/config/scanner.toml"
MONITOR_FILE="$KLAMS_ROOT/config/monitor.toml"

generate_token() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 32
    else
        head -c 32 /dev/urandom | xxd -p -c 64
    fi
}

generate_password() {
    if command -v openssl >/dev/null 2>&1; then
        openssl rand -hex 24
    else
        head -c 24 /dev/urandom | xxd -p -c 48
    fi
}

if [[ -f "$TOKEN_FILE" || -f "$ENV_FILE" ]]; then
    echo "==> Existing config detected; skipping render."
    echo "    klams.toml:   $TOKEN_FILE"
    echo "    compose.env:  $ENV_FILE"
    echo "    scanner.toml: $SCANNER_FILE"
    echo "    monitor.toml: $MONITOR_FILE"
else
    OPERATOR_TOKEN="$(generate_token)"
    SCANNER_TOKEN="$(generate_token)"
    MONITOR_TOKEN="$(generate_token)"
    PG_PASSWORD="$(generate_password)"

    echo "==> Rendering $TOKEN_FILE"
    cp "$EXAMPLE_TOML" "$TOKEN_FILE"
    sed -i \
        -e "s|postgres://klams:changeme@127.0.0.1:5432/klams|postgres://klams:$PG_PASSWORD@127.0.0.1:5432/klams|" \
        "$TOKEN_FILE"
    # Sprint 034 (#773): the example ships every token form commented out
    # (#670), so a rendered config MUST append a grant or the service
    # refuses to start (AuthConfigError::NoTokens) — which is exactly what
    # this script silently produced between 032 and 034 (its old sed
    # pattern matched nothing in klams.example.toml). Emit a scoped,
    # attributed operator grant; `manage` requires `agent_name` (#703).
    # Sprint 035 (#776): scanner + monitor get their own read+write
    # grants here too, matching the rendered scanner.toml/monitor.toml.
    cat >>"$TOKEN_FILE" <<TOML

# Rendered by provision-storage-root.sh — the operator's starting
# credential (read+write+manage, attributed to "operator"). Add more
# grants per consumer as needed; the service reloads [[auth.tokens]]
# on SIGHUP. For admin verbs (restore / hard-delete), add an explicit
# admin grant — see docs/auth.md.
[[auth.tokens]]
token      = "$OPERATOR_TOKEN"
scopes     = ["read", "write", "manage"]
label      = "operator"
agent_name = "operator"

# Daemon grants (rendered with scanner.toml / monitor.toml): write-only
# identities, deliberately without \`manage\` — they curate nothing.
[[auth.tokens]]
token      = "$SCANNER_TOKEN"
scopes     = ["read", "write"]
label      = "scanner"
agent_name = "klams-scanner"

[[auth.tokens]]
token      = "$MONITOR_TOKEN"
scopes     = ["read", "write"]
label      = "monitor"
agent_name = "klams-monitor"
TOML
    chmod 600 "$TOKEN_FILE"

    # Sprint 035 (#776): render scanner.toml and monitor.toml with url +
    # token filled, instead of leaving the operator to copy the examples
    # and discover the fields the hard way. The scanner's \`roots\`
    # placeholder is left as-is on purpose — the scanner refuses to
    # start until it points at real paths.
    echo "==> Rendering $SCANNER_FILE"
    cp "$EXAMPLE_SCANNER" "$SCANNER_FILE"
    sed -i \
        -e "s|CHANGE-ME-scanner-bearer|$SCANNER_TOKEN|" \
        "$SCANNER_FILE"
    chmod 600 "$SCANNER_FILE"

    echo "==> Rendering $MONITOR_FILE"
    cp "$EXAMPLE_MONITOR" "$MONITOR_FILE"
    sed -i \
        -e "s|CHANGE-ME-monitor-bearer|$MONITOR_TOKEN|" \
        "$MONITOR_FILE"
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
if [[ -n "${OPERATOR_TOKEN:-}" ]]; then
    cat <<EOF

Operator bearer token (also in $TOKEN_FILE, mode 600):

    $OPERATOR_TOKEN

EOF
fi
cat <<EOF
Next steps (docs/install.md walks these in full):

  1. Review and adjust (CPU-only hosts: see the decision tree in
     docs/install.md — TEI image tag, model, vector_dim all change):
       \$EDITOR $TOKEN_FILE
       \$EDITOR $ENV_FILE

  2. Bring up the backing services (postgres, qdrant, tei, reranker):
       cd $REPO_ROOT/deploy
       docker compose --env-file $ENV_FILE up -d

  3. Build and run klams-service:
       cd $REPO_ROOT
       cargo build --release -p klams-service
       KLAMS_CONFIG=$TOKEN_FILE ./target/release/klams-service

  4. Prove the install end-to-end (uses the operator token):
       KLAMS_TOKEN=<token> just smoke

  5. To index your files, set \`roots\` in $SCANNER_FILE
     and run the scanner (its token is already rendered).

EOF
