#!/usr/bin/env sh
# Post a minimal UserFact to klams.
#
# Env (with defaults):
#   KLAMS_URL    - base URL of the klams service. Default: http://127.0.0.1:7777
#   KLAMS_AGENT  - declared identity. Default: declared-test-agent
#                  (matches the docker test stack). Not a secret.
#
# Deps:
#   curl   - required
#   jq     - optional; raw response printed when missing
#
# Exit codes:
#   0  - HTTP 200 (canonical write)
#   1  - HTTP non-200 or curl error
#   2  - missing curl

set -eu

URL=${KLAMS_URL:-http://127.0.0.1:7777}
AGENT=${KLAMS_AGENT:-declared-test-agent}

if ! command -v curl >/dev/null 2>&1; then
    echo "curl is required" >&2
    exit 2
fi

payload='{"type":"UserFact","payload":{"name":"Ken","host":"kubs0"},"source":"User"}'

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

http_code=$(
    curl -sS -o "$tmp" -w '%{http_code}' \
        -H "X-Homelab-Agent: $AGENT" \
        -H 'Content-Type: application/json' \
        -X POST \
        --data "$payload" \
        "$URL/memory/facts"
)

if command -v jq >/dev/null 2>&1; then
    jq . <"$tmp"
else
    cat "$tmp"
    echo
fi

if [ "$http_code" != "200" ]; then
    echo "klams returned $http_code (expected 200)" >&2
    exit 1
fi
