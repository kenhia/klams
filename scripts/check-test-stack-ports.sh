#!/usr/bin/env bash
# Assert every test-stack service actually has its host port published.
#
# Sprint 055 (#3267): when a host port is taken, compose can start the
# container without the mapping and still report it healthy — measured on
# kubs0, where TEI came up "Healthy" with nothing on 127.0.0.1:57070 and
# the suite then failed as EMBEDDING_UNAVAILABLE. The ports now sit above
# the kernel's ephemeral range (32768-60999, the same on GitHub's Linux
# runners), and this check turns any recurrence into a named failure.
#
# One implementation, two callers: `just test-stack-up` (after `--wait`)
# and CI's `.github/actions/test-stack` (after `up -d` — a mapping is fixed
# when the container starts, so it need not wait for health).
#
# Usage: scripts/check-test-stack-ports.sh [compose-file]
# Exit:  0 all mapped; 1 one or more services unmapped (each named on stderr).

set -euo pipefail

compose_file="${1:-tests/docker-compose.test.yml}"

# service:container-port:host-port — keep in step with the compose file.
mappings=(
    postgres:5432:61400
    qdrant:6333:61401
    qdrant:6334:61402
    tei:80:61403
    reranker:80:61404
)

fail=0
for spec in "${mappings[@]}"; do
    IFS=: read -r svc inner want <<<"$spec"
    got="$(docker compose -f "$compose_file" port "$svc" "$inner" 2>/dev/null || true)"
    if [[ "$got" == "127.0.0.1:$want" ]]; then
        echo "mapped: $svc :$inner -> 127.0.0.1:$want"
    else
        echo "error: test stack service '$svc' has no host port $want published for :$inner (got '${got:-nothing}')" >&2
        echo "       is 127.0.0.1:$want taken? check: ss -tanp | grep :$want" >&2
        echo "       then: docker compose -f $compose_file up -d --wait --force-recreate $svc" >&2
        fail=1
    fi
done
exit "$fail"
