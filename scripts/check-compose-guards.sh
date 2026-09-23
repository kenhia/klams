#!/usr/bin/env bash
# Sprint 054 (#2711) — prove deploy/docker-compose.yml refuses to render
# with any of its required variables unset or empty.
#
# Without the `${VAR:?}` guards, an unloaded environment interpolates to
# the empty string: KLAMS_DATA_ROOT empty binds /postgres and /qdrant —
# fresh, empty host directories — in place of the memory store, and
# `up -d` would recreate the containers onto them. This check renders
# the compose offline (`docker compose config`, no daemon contact) with a
# complete dummy environment, then blanks each variable in turn and
# asserts the render fails *and* names that variable. A render that fails
# for some other reason is not a pass: an empty result and a suppressed
# failure are indistinguishable unless the failure is checked for being
# the right one.
#
# Exit 0 when every guard holds; non-zero with the failing variable named.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE=(docker compose -f "$REPO_ROOT/deploy/docker-compose.yml"
         -f "$REPO_ROOT/deploy/docker-compose.gpu.yml")

REQUIRED=(
    KLAMS_DATA_ROOT
    POSTGRES_IMAGE_TAG
    POSTGRES_PASSWORD
    QDRANT_IMAGE_TAG
    TEI_IMAGE_TAG
    TEI_MODEL_ID
    RERANKER_MODEL_ID
)

if ! docker compose version >/dev/null 2>&1; then
    echo "error: 'docker compose' is not available; install the compose plugin" >&2
    exit 1
fi

# Run with a scrubbed environment so an operator's shell (or a sourced
# compose.env) cannot make a missing guard look present. COMPOSE_* are
# cleared too, and the project directory is pinned so no stray .env is
# read.
render() {
    env -i PATH="$PATH" HOME="${HOME:-/tmp}" "$@" \
        "${COMPOSE[@]}" --project-directory "$REPO_ROOT/deploy" \
        --profile observability config --quiet
}

dummy_env() {
    local skip="$1" v
    for v in "${REQUIRED[@]}"; do
        if [[ "$v" == "$skip" ]]; then
            printf '%s=\n' "$v"
        elif [[ "$v" == KLAMS_DATA_ROOT ]]; then
            # A bind source must be absolute; never touched by `config`.
            printf '%s=/nonexistent/klams-data\n' "$v"
        else
            printf '%s=dummy-%s\n' "$v" "$v"
        fi
    done
}

# Control: the complete environment must render, or every refusal below
# would be meaningless.
mapfile -t full < <(dummy_env "")
if ! out=$(render "${full[@]}" 2>&1); then
    echo "FAIL: compose does not render with every required variable set:" >&2
    echo "$out" >&2
    exit 1
fi
echo "ok   renders with all ${#REQUIRED[@]} variables set"

fail=0
for var in "${REQUIRED[@]}"; do
    # Empty and unset are both refused by `:?`; test the two separately.
    mapfile -t empty < <(dummy_env "$var")
    mapfile -t unset_ < <(dummy_env "$var" | grep -v "^$var=")
    for mode in empty unset; do
        if [[ "$mode" == empty ]]; then args=("${empty[@]}"); else args=("${unset_[@]}"); fi
        if out=$(render "${args[@]}" 2>&1); then
            echo "FAIL: $var $mode — compose rendered anyway (no :? guard)" >&2
            fail=1
        elif ! grep -q "$var" <<<"$out"; then
            echo "FAIL: $var $mode — refused, but not by its guard:" >&2
            echo "$out" >&2
            fail=1
        else
            echo "ok   $var $mode refused"
        fi
    done
done

exit "$fail"
