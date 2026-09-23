#!/bin/sh
# sprint-003 T045 — install klams-{service,scanner,monitor} under systemd.
#
# Idempotent. Supports --dry-run.
#
# Steps (in order):
#   1. Ensure system user `klams` exists.
#   2. Ensure /var/lib/klams and /etc/klams exist, owned by klams.
#   3. For each binary in BIN_LIST: stage to /tmp, rotate any existing
#      `/usr/local/bin/<bin>` to `<bin>.prev`, mv-into-place atomically.
#   4. Install unit + timer files into /etc/systemd/system.
#   5. systemctl daemon-reload + enable --now the units.
#   5a. enable --now klams-stack.service, if /etc/klams/compose.env exists.
#
# Required deps: `docker.service` must exist on the host. Postgres, Qdrant,
# and the embeddings backend run as Docker containers (see compose files),
# so `klams-service.service` declares `After=/Wants=docker.service`. We do
# not install Docker for you; we only verify the unit is on disk so the
# After=/Wants= in klams-service.service can be satisfied.

set -eu

DRY_RUN=0
case "${1:-}" in
    --dry-run) DRY_RUN=1 ;;
    "") ;;
    *) echo "usage: $0 [--dry-run]" >&2; exit 2 ;;
esac

SCRIPT_DIR=$(cd -- "$(dirname -- "$0")" && pwd -P)
BIN_SRC_DIR=${BIN_SRC_DIR:-"$SCRIPT_DIR/../target/release"}
BIN_DST_DIR=/usr/local/bin
SYSTEMD_DIR=/etc/systemd/system
STATE_DIR=/var/lib/klams
CONFIG_DIR=/etc/klams
USER_NAME=klams
GROUP_NAME=klams

BIN_LIST="klams-service klams-scanner klams-monitor"
UNIT_LIST="klams-service.service klams-scanner.service klams-scanner.timer klams-monitor.service klams-stack.service"

# Sprint 051 — the per-host secrets file is k-homelab's; klams neither creates
# nor requires it. The drop-in that reads it is installed only where the file
# already exists (step 4a), so a host without k-homelab keeps a unit that
# starts.
KHOMELAB_SECRETS=/etc/khomelab/secrets.env
DROPIN_NAME=10-khomelab-secrets.conf
DROPIN_SRC="$SCRIPT_DIR/klams-monitor.service.d/$DROPIN_NAME"
DROPIN_DST_DIR="$SYSTEMD_DIR/klams-monitor.service.d"
ENABLE_LIST="klams-service.service klams-scanner.timer klams-monitor.service"

# Sprint 054 (#2711) — the compose stack's unit. Its environment is the
# operator's to write (it carries the Postgres password), so the unit is
# always installed but enabled only once that file exists (step 5a).
STACK_UNIT=klams-stack.service
STACK_ENV=$CONFIG_DIR/compose.env

say() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '[dry-run] %s\n' "$*"
    else
        printf '+ %s\n' "$*"
    fi
}

run() {
    say "$*"
    if [ "$DRY_RUN" -eq 0 ]; then
        eval "$@"
    fi
}

fail() {
    printf 'ERROR: %s\n' "$1" >&2
    exit 1
}

# --- 0. Pre-flight checks -------------------------------------------------

# docker.service must be known to systemd: the datastores run as Docker
# containers and klams-service.service declares After=/Wants=docker.service.
if ! systemctl cat docker.service >/dev/null 2>&1; then
    fail "docker.service not found on this host; install Docker first"
fi

for bin in $BIN_LIST; do
    if [ ! -x "$BIN_SRC_DIR/$bin" ]; then
        fail "missing binary $BIN_SRC_DIR/$bin (run 'cargo build --release' first)"
    fi
done

for unit in $UNIT_LIST; do
    if [ ! -f "$SCRIPT_DIR/$unit" ]; then
        fail "missing unit file $SCRIPT_DIR/$unit"
    fi
done

if [ ! -f "$DROPIN_SRC" ]; then
    fail "missing drop-in $DROPIN_SRC"
fi

# --- 1. User + group ------------------------------------------------------

if getent passwd "$USER_NAME" >/dev/null 2>&1; then
    say "user $USER_NAME exists"
else
    run "useradd --system --no-create-home --shell /usr/sbin/nologin $USER_NAME"
fi

# --- 2. State + config dirs ----------------------------------------------

run "install -d -o $USER_NAME -g $GROUP_NAME -m 0750 $STATE_DIR"
run "install -d -o $USER_NAME -g $GROUP_NAME -m 0750 $CONFIG_DIR"

# --- 3. Binaries (rotate prev) -------------------------------------------

STAGE_DIR="/tmp/klams-stage-$$"
run "mkdir -p $STAGE_DIR"
for bin in $BIN_LIST; do
    run "install -m 0755 $BIN_SRC_DIR/$bin $STAGE_DIR/$bin"
done

for bin in $BIN_LIST; do
    dst="$BIN_DST_DIR/$bin"
    if [ -f "$dst" ]; then
        run "mv -f $dst $dst.prev"
    fi
    run "mv -f $STAGE_DIR/$bin $dst"
done

run "rm -rf $STAGE_DIR"

# --- 4. Unit files --------------------------------------------------------

for unit in $UNIT_LIST; do
    run "install -m 0644 $SCRIPT_DIR/$unit $SYSTEMD_DIR/$unit"
done

# --- 4a. Per-host secrets drop-in (sprint 051) ---------------------------
#
# klams-monitor's kpidash reporter needs REDISCLI_AUTH. On a homelab host that
# comes from the file k-homelab renders; everywhere else the unit must still
# start, so the drop-in goes in only when the file is already there. The
# drop-in has no `-` on its EnvironmentFile, so once installed a missing
# secrets file is a failed unit rather than a monitor publishing nothing.

if [ -f "$KHOMELAB_SECRETS" ]; then
    say "found $KHOMELAB_SECRETS - installing $DROPIN_NAME"
    run "install -d -m 0755 $DROPIN_DST_DIR"
    run "install -m 0644 $DROPIN_SRC $DROPIN_DST_DIR/$DROPIN_NAME"
else
    printf 'note: %s absent; skipping %s.\n' "$KHOMELAB_SECRETS" "$DROPIN_NAME"
    printf '      klams-monitor will start without REDISCLI_AUTH. Set\n'
    printf '      [kpidash].password in monitor.toml if you want dashboard reporting.\n'
fi

# --- 5. daemon-reload + enable ------------------------------------------

run "systemctl daemon-reload"
for unit in $ENABLE_LIST; do
    run "systemctl enable --now $unit"
done

# --- 5a. The compose stack (sprint 054) ----------------------------------
#
# Enabled only when its env file exists: without it the unit fails on
# start by design, and set -e would abort the install on a host that
# simply has not moved compose.env into /etc/klams yet. `enable --now`
# on a stack that is already up runs `docker compose up -d`, which
# recreates nothing when compose.env matches the running containers —
# check that first with the dry run in docs/setup.md.

if [ -f "$STACK_ENV" ]; then
    run "systemctl enable --now $STACK_UNIT"
else
    printf 'note: %s absent; %s installed but not enabled.\n' "$STACK_ENV" "$STACK_UNIT"
    printf '      Create it (root 0600, see deploy/compose.env.example), then\n'
    printf '      systemctl enable --now %s\n' "$STACK_UNIT"
fi

printf 'done.\n'
