#!/usr/bin/env bash
# Reap test detritus from the docker-compose test stack.
#
# Sprint 031 (#687/#679). The integration stack is long-lived — the one
# on kubs0 has been up for weeks — and two kinds of rubbish pile up in
# it:
#
#   1. The SHARED `knowledge_items_test` collection. Every
#      `TestServer::spawn()` seeds into it and nothing ever empties it,
#      so it grows without bound. Presence assertions don't care; the
#      ranking ones starve, because a top-10 page eventually holds
#      nothing but stale near-duplicate seeds. That is how
#      `phase4_hybrid_retrieval::literal_and_paraphrase_share_results`
#      came to fail on an unmodified `main` during sprint 030.
#
#   2. ORPHANED per-test resources — `klams_test_<uuid>` Qdrant
#      collections and Postgres schemas from `spawn_isolated`. Cleanup
#      is explicit, so a test that panics before `cleanup()` leaves its
#      pair behind. 107 orphaned collections had accumulated by 031.
#
# Both are dropped and recreated on next use, so this is safe to run
# before any suite — but NOT while one is running.
#
# It deliberately does NOT touch the loaded scale fixture in the
# service's own collections, the Postgres `public` schema, or the
# stack's volumes: `just backup-size` depends on that fixture and
# reloading it takes minutes.
#
# Usage: scripts/reset-test-stack.sh
# Env:   TEST_QDRANT_HTTP_URL (default http://127.0.0.1:56333)
#        TEST_PG_CONTAINER    (default klams-test-postgres-1)
set -euo pipefail

qdrant="${TEST_QDRANT_HTTP_URL:-http://127.0.0.1:56333}"
pg_container="${TEST_PG_CONTAINER:-klams-test-postgres-1}"

if ! curl -fsS "$qdrant/readyz" >/dev/null 2>&1; then
    echo "reset-test-stack: qdrant unreachable at $qdrant" >&2
    echo "  bring the stack up: docker compose -f tests/docker-compose.test.yml up -d" >&2
    exit 1
fi

# --- Qdrant: the shared collection, then the orphans -----------------
collections=$(curl -fsS "$qdrant/collections" |
    python3 -c 'import json,sys; print("\n".join(c["name"] for c in json.load(sys.stdin)["result"]["collections"]))')

dropped=0
while read -r name; do
    [[ -z "$name" ]] && continue
    case "$name" in
        knowledge_items_test | klams_test_*) ;;
        *) continue ;;
    esac
    curl -fsS -X DELETE "$qdrant/collections/$name" >/dev/null
    dropped=$((dropped + 1))
done <<<"$collections"
echo "reset-test-stack: dropped $dropped qdrant test collection(s)"

# --- Postgres: orphaned per-test schemas ----------------------------
# `public` is never touched — the shared-database tests still live there.
if docker ps --format '{{.Names}}' | grep -qx "$pg_container"; then
    schemas=$(docker exec "$pg_container" psql -U klams -d klams -tAc \
        "SELECT schema_name FROM information_schema.schemata WHERE schema_name LIKE 'klams\_test\_%'")
    n=0
    while read -r schema; do
        [[ -z "$schema" ]] && continue
        docker exec "$pg_container" psql -U klams -d klams -qc \
            "DROP SCHEMA IF EXISTS $schema CASCADE" >/dev/null
        n=$((n + 1))
    done <<<"$schemas"
    echo "reset-test-stack: dropped $n orphaned postgres schema(s)"
else
    echo "reset-test-stack: container $pg_container not running, skipping postgres sweep" >&2
fi
