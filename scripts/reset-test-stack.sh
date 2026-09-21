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
#   3. The `klams_backup_*` / `klams_restore_*` fixture collections.
#      Restoring a snapshot into a collection leaves qdrant 1.18 unable
#      to snapshot it again ("Failed to get_snapshot_creator"), so one
#      restore run poisons every later one. The tests now drop these
#      themselves, but sweeping here heals a stack wedged by an older
#      build.
#
# Both are dropped and recreated on next use, so this is safe to run
# before any suite — but NOT while one is running.
#
# It WAITS for the stack to be ready before sweeping (#2283). `docker
# compose up -d` returns when containers are *started*, not when their
# healthchecks pass — measured on kubs0: `up -d` returns after 1s, and
# `up -d --wait` after 16s. AGENTS.md pairs `up -d` with
# `just test-integration` as consecutive commands, so the one-shot probe
# this used to do failed that documented sequence every time, and said
# "bring the stack up" — the command the operator had just run.
#
# It deliberately does NOT touch the loaded scale fixture in the
# service's own collections, the Postgres `public` schema, or the
# stack's volumes: `just backup-size` depends on that fixture and
# reloading it takes minutes.
#
# Usage: scripts/reset-test-stack.sh
# Env:   TEST_QDRANT_HTTP_URL  (default http://127.0.0.1:56333)
#        TEST_QDRANT_CONTAINER (default klams-test-qdrant-1) — used only
#                              to tell "stack is down" from "wedged"
#        TEST_PG_CONTAINER     (default klams-test-postgres-1)
#        TEST_STACK_WAIT_SECS  (default 60) — readiness budget; 0 waits
#                              not at all, restoring the old behaviour
set -euo pipefail

qdrant="${TEST_QDRANT_HTTP_URL:-http://127.0.0.1:56333}"
qdrant_container="${TEST_QDRANT_CONTAINER:-klams-test-qdrant-1}"
pg_container="${TEST_PG_CONTAINER:-klams-test-postgres-1}"
wait_secs="${TEST_STACK_WAIT_SECS:-60}"

# --- Wait for ready, not merely started ------------------------------
container_running() {
    docker ps --format '{{.Names}}' | grep -qx "$1"
}

qdrant_ready() {
    curl -fsS --max-time 2 "$qdrant/readyz" >/dev/null 2>&1
}

# Postgres is checked only when its container is actually running: the
# sweep below already tolerates its absence (someone pointing
# TEST_QDRANT_HTTP_URL at a standalone qdrant has no postgres to wait
# for), and waiting 60s to then print "skipping postgres sweep" would
# turn that supported case into a stall.
pg_ready() {
    container_running "$pg_container" || return 0
    docker exec "$pg_container" pg_isready -U klams >/dev/null 2>&1
}

stack_ready() {
    qdrant_ready && pg_ready
}

start=$(date +%s)
deadline=$((start + wait_secs))
announced=0
until stack_ready; do
    if (($(date +%s) >= deadline)); then
        if ! qdrant_ready; then
            echo "reset-test-stack: qdrant at $qdrant was not ready within ${wait_secs}s" >&2
            if container_running "$qdrant_container"; then
                echo "  container $qdrant_container IS running, so it is wedged rather than absent:" >&2
                echo "    docker compose -f tests/docker-compose.test.yml logs qdrant" >&2
            else
                echo "  no container named $qdrant_container is running — the stack is down:" >&2
                echo "    docker compose -f tests/docker-compose.test.yml up -d --wait" >&2
                echo "  (or TEST_QDRANT_HTTP_URL points somewhere there is no qdrant)" >&2
            fi
        else
            echo "reset-test-stack: postgres in $pg_container was not accepting" \
                "connections within ${wait_secs}s" >&2
            echo "    docker compose -f tests/docker-compose.test.yml logs postgres" >&2
        fi
        exit 1
    fi
    if ((announced == 0)); then
        echo "reset-test-stack: stack not ready yet, waiting up to ${wait_secs}s" >&2
        announced=1
    fi
    sleep 1
done
waited=$(($(date +%s) - start))
if ((waited > 0)); then
    echo "reset-test-stack: stack ready after ${waited}s" >&2
fi

# --- Qdrant: the shared collection, then the orphans -----------------
collections=$(curl -fsS "$qdrant/collections" |
    python3 -c 'import json,sys; print("\n".join(c["name"] for c in json.load(sys.stdin)["result"]["collections"]))')

dropped=0
while read -r name; do
    [[ -z "$name" ]] && continue
    case "$name" in
        knowledge_items_test | klams_test_* | klams_backup_* | klams_restore_*) ;;
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
