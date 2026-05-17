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

if [[ ! -f "$EXAMPLE_ENV" || ! -f "$EXAMPLE_TOML" ]]; then
    echo "error: expected $EXAMPLE_ENV and $EXAMPLE_TOML to exist" >&2
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
else
    BEARER_TOKEN="$(generate_token)"
    PG_PASSWORD="$(generate_password)"

    echo "==> Rendering $TOKEN_FILE"
    cp "$EXAMPLE_TOML" "$TOKEN_FILE"
    sed -i \
        -e "s|changeme-rendered-by-provision-script|$BEARER_TOKEN|" \
        -e "s|postgres://klams:changeme@127.0.0.1:5432/klams|postgres://klams:$PG_PASSWORD@127.0.0.1:5432/klams|" \
        "$TOKEN_FILE"
    chmod 600 "$TOKEN_FILE"

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

Done. Next steps:

  1. Review and adjust:
       \$EDITOR $TOKEN_FILE
       \$EDITOR $ENV_FILE

  2. Bring up service dependencies:
       cd $REPO_ROOT/deploy
       docker compose --env-file $ENV_FILE up -d postgres qdrant tei

  3. Build and run klams-service:
       cd $REPO_ROOT
       cargo build --release -p klams-service
       KLAMS_CONFIG=$TOKEN_FILE ./target/release/klams-service

EOF
