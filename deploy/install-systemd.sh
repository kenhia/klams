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
#
# Required deps: `postgresql.service` must exist on the host. We do not
# install it for you; we only verify the unit file is on disk so the
# After=/Requires= in klams-service.service can be satisfied.

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
UNIT_LIST="klams-service.service klams-scanner.service klams-scanner.timer klams-monitor.service"
ENABLE_LIST="klams-service.service klams-scanner.timer klams-monitor.service"

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

# postgresql.service must be known to systemd (per plan.md Constraints).
if ! systemctl cat postgresql.service >/dev/null 2>&1; then
    fail "postgresql.service not found on this host; install postgres first"
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

# --- 5. daemon-reload + enable ------------------------------------------

run "systemctl daemon-reload"
for unit in $ENABLE_LIST; do
    run "systemctl enable --now $unit"
done

printf 'done.\n'
