#!/usr/bin/env bash
# sprint 006 T044 — backup status_hook fixture.
#
# Writes the JSON document piped on stdin to
#   ${KLAMS_HOOK_OUT_DIR:-/tmp}/klams-hook-${KLAMS_BACKUP_EVENT}-${KLAMS_BACKUP_RUN_ID}.json
# so tests can assert the contents asynchronously.
set -euo pipefail
: "${KLAMS_BACKUP_EVENT:?KLAMS_BACKUP_EVENT must be set}"
: "${KLAMS_BACKUP_RUN_ID:?KLAMS_BACKUP_RUN_ID must be set}"
out_dir="${KLAMS_HOOK_OUT_DIR:-/tmp}"
mkdir -p "$out_dir"
cat > "$out_dir/klams-hook-${KLAMS_BACKUP_EVENT}-${KLAMS_BACKUP_RUN_ID}.json"
