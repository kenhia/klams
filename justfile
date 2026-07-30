# klams — one-command developer inner loop.
#
# `just --list`  ─ enumerate recipes
# `just gate`    ─ constitution pre-commit gate (also what CI runs)
# `just verify`  ─ end-to-end functional smoke against a running service

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]
set positional-arguments
# Machine-local values (KLAMS_TOKEN, KLAMS_MIND_DIR, VIEWPORT_HOST, …)
# can live in a gitignored `.env` at the repo root instead of the shell
# environment (sprint 035, #776).
set dotenv-load := true

# Default recipe shows the menu so a bare `just` is friendly.
default:
    @just --list

# Service URL + bearer token used by `verify` and `health`. Override
# in the environment when pointing at a non-local stack.
#
# KLAMS_TOKEN has no default on purpose (sprint 031, #682). It used to
# fall back to `dev-token`, so forgetting to set it produced a 401 that
# read like an auth regression instead of "you didn't set the variable".
klams_url     := env_var_or_default('KLAMS_URL',   'http://127.0.0.1:7777')
klams_token   := env_var_or_default('KLAMS_TOKEN', '')
compose_file  := 'deploy/docker-compose.yml'

# Config path, resolved the same way klams-service does (sprint 034,
# #775): $KLAMS_CONFIG wins; else the storage-root default if present;
# else $XDG_CONFIG_HOME/klams/klams.toml (~/.config fallback).
klams_config  := env_var_or_default('KLAMS_CONFIG', if path_exists('/ai/klams/config/klams.toml') == 'true' { '/ai/klams/config/klams.toml' } else { env_var_or_default('XDG_CONFIG_HOME', home_directory() / '.config') / 'klams/klams.toml' })

# klams-mind checkout — it owns the retrieval eval suite + runner (the
# TOML and the Python harness live there; the gate lives here, because
# klams is what regresses). No default (sprint 035, #776 — the #682
# pattern): a sibling-directory guess that doesn't exist fails as a
# confusing uv error instead of "you didn't set the variable". The
# `eval` recipes check and say what to set.
klams_mind    := env_var_or_default('KLAMS_MIND_DIR', '')

# Windows host + deploy target used by `viewport-deploy`. VIEWPORT_HOST
# has no default for the same reason (it used to be one specific
# machine); the recipe fails loudly when unset.
viewport_host       := env_var_or_default('VIEWPORT_HOST',       '')
viewport_deploy_dir := env_var_or_default('VIEWPORT_DEPLOY_DIR', 'c:\tools\bin')

# Bring the Postgres+Qdrant+TEI+reranker stack up in the background.
compose-up:
    docker compose -f {{compose_file}} up -d

# Alias for `compose-up` — kept so quickstart-style operator docs can
# distinguish a throwaway test stack from a long-lived one.
compose-up-test: compose-up

# Stop and remove the stack (keeps volumes).
compose-down:
    docker compose -f {{compose_file}} down

# Force a clean rebuild of all compose images.
compose-rebuild:
    docker compose -f {{compose_file}} down
    docker compose -f {{compose_file}} build --no-cache
    docker compose -f {{compose_file}} up -d

# Release build of the service binary.
build:
    cargo build -p klams-service --release

# Run the service in the foreground; logs go to stderr.
run:
    cargo run -p klams-service 2>&1

# Quickstart-friendly alias for `run`.
service-run: run

# sprint 007 — validate [auth] / [backup] / [decay] in {{klams_config}}
# without contacting any backend. Exits 0 on success, 2 on the first
# error; warnings go to stderr.
service-validate-config:
    KLAMS_CONFIG={{klams_config}} \
        cargo run --quiet -p klams-service -- --validate-config

# Workspace-wide tests (excludes #[ignore]'d cases).
test:
    cargo test --workspace

# Sprint 009 — run the loopback half-close soak harness against the
# configured target (defaults to 127.0.0.1:7777, 10m duration, 32
# concurrent half-open connections at 4/s). Forward extra `ARGS`
# (e.g. `--duration 18h`) directly to the binary.
soak *ARGS:
    cargo run --release -p klams-soak -- {{ARGS}}

# Constitution pre-commit gate — fail-fast on fmt, clippy, or tests.
# Mirrors CI's `service` job exactly. NOTE: the root workspace does NOT
# include the viewport (viewport/src-tauri is its own Cargo workspace),
# so this recipe does not gate viewport code — use `gate-viewport` for
# that, or `gate-all` for both. A change to a `klams_types` shape the
# viewport consumes can pass `gate` and still break CI's viewport job.
# Note: excludes `--all-features` which gates off `scale-fixture` (an intentionally
# heavy fixture for multi-minute loads); that feature is checked only in targeted tests.
gate:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace

# Mirrors CI's `viewport` job exactly (svelte + tauri). Needs pnpm and
# the Tauri Linux deps (libwebkit2gtk-4.1-dev etc.); slower than `gate`,
# so it's separate rather than folded in. Run it whenever a change
# touches the viewport or a shared type the viewport serializes.
gate-viewport:
    cd viewport && pnpm install --frozen-lockfile && pnpm check && pnpm build
    cd viewport/src-tauri && cargo fmt --all -- --check
    cd viewport/src-tauri && cargo clippy --all-targets --features custom-protocol -- -D warnings
    cd viewport/src-tauri && cargo test --features custom-protocol

# The complete gate — both CI jobs. Use before shipping anything that
# spans the service and the viewport.
gate-all: gate gate-viewport

# Sprint 031 (#679/#687/#646) — the docker-gated integration suite,
# which `gate` deliberately excludes. Until 031 there was no recipe for
# this at all: the only place it ran was a main-branch-only CI step, so
# "how do I run the ignored tests" had no answer you could `just`.
#
# Sweeps the test stack first (see scripts/reset-test-stack.sh — a
# long-lived stack accumulates seeds until ranking assertions starve),
# then runs at DEFAULT parallelism. The `--test-threads=1` this used to
# need is gone with the shared-table race (#679); if you find yourself
# reaching for it again, something regressed — fix that instead.
#
# Requires `docker compose -f tests/docker-compose.test.yml up -d`.
test-integration *ARGS:
    ./scripts/reset-test-stack.sh
    TEST_DATABASE_URL=postgres://klams:klams_test@127.0.0.1:55432/klams \
    TEST_QDRANT_URL=http://127.0.0.1:56334 \
    TEST_TEI_URL=http://127.0.0.1:57070 \
    TEST_OPENAI_EMBED_URL=http://127.0.0.1:57070/v1 \
    TEST_OPENAI_EMBED_MODEL=BAAI/bge-small-en-v1.5 \
    TEST_RERANKER_URL=http://127.0.0.1:57071 \
        cargo test --workspace -- --ignored {{ARGS}}

# Quick liveness probe + light verification round-trip.
#
# The `@` on token-carrying recipes is not cosmetic: without it `just`
# echoes the expanded command line, printing the bearer token to the
# terminal (and to any CI log) on every run.
health:
    @KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        bash scripts/verify-mvp.sh --light

# sprint 007 — apply pending SQL migrations against the configured
# Postgres without starting the HTTP server. Idempotent — `sqlx`
# tracks applied versions in `_sqlx_migrations` so reruns are no-ops.
db-migrate:
    KLAMS_CONFIG={{klams_config}} \
        cargo run --quiet -p klams-service -- --migrate-only

# Shell into the compose Postgres as the klams user. Forwards extra
# args, so `just db-psql -c "SELECT 1"` and `just db-psql -t -c "..."`
# both work.
db-psql *ARGS:
    @docker exec -i klams-postgres psql -U klams -d klams "$@"

# Sprint 026 (#643) — the retrieval regression bar.
#
# The suite lives in klams-mind (it owns the TOML and the runner), but the
# gate belongs here: klams is what regresses. Exits non-zero on a
# REGRESSION — a query marked `expect = "pass"` that stopped passing.
# Queries marked `known_open` are failing against tracked work (klams#628
# curated-beats-bulk, the fence-unaware chunker) and do NOT fail the run;
# if one starts passing, the report says so and it should be promoted.
#
# Not folded into `gate`: it needs a live klams with the real corpus, so
# it is a pre-deploy check rather than a per-commit one. Run it before
# and after a deploy that touches retrieval.
eval:
    @if [ -z '{{klams_mind}}' ]; then echo 'eval: set KLAMS_MIND_DIR to your klams-mind checkout (owns the eval suite + runner)' >&2; exit 1; fi
    @KLAMS_TOKEN={{klams_token}} KLAMS_URL={{klams_url}} \
        uv run --project {{klams_mind}} klams-mind eval run \
        {{klams_mind}}/evals/suites/homelab-retrieval.toml

# Same, writing the markdown report somewhere durable — use this to
# capture a before/after around a retrieval change or the corpus reset.
eval-report OUT:
    @if [ -z '{{klams_mind}}' ]; then echo 'eval-report: set KLAMS_MIND_DIR to your klams-mind checkout (owns the eval suite + runner)' >&2; exit 1; fi
    @KLAMS_TOKEN={{klams_token}} KLAMS_URL={{klams_url}} \
        uv run --project {{klams_mind}} klams-mind eval run \
        {{klams_mind}}/evals/suites/homelab-retrieval.toml --out {{OUT}}

# Full SC-001..SC-009 functional smoke (slower than `health`).
verify:
    @KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        bash scripts/verify-mvp.sh

# Sprint 035 (#779) — the first-run smoke docs/install.md ends with:
# proves an empty install end-to-end (health → fact round-trip →
# knowledge write/embed/search → error handling → metrics) and closes
# with a plain-language verdict. Valid on a completely empty store.
#   KLAMS_TOKEN=<operator token> just smoke
smoke:
    @KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        bash scripts/verify-mvp.sh --first-run

# Cross-compile the viewport for Windows (requires cargo-xwin).
# Builds the SvelteKit frontend first — tauri's `generate_context!`
# panics at compile time if `viewport/build/` doesn't exist.
viewport-build:
    cd viewport && pnpm install --frozen-lockfile && pnpm build
    cd viewport/src-tauri && cargo xwin build --release --target x86_64-pc-windows-msvc

# Cross-compile the viewport and ship klams-viewport.exe to the
# Windows host (cleo) at VIEWPORT_DEPLOY_DIR. Override VIEWPORT_HOST /
# VIEWPORT_DEPLOY_DIR to target a different machine/path.
#
# History worth keeping: this shipped as a *pull*
# (`ssh cleo "scp kubs0:... dest"`) because cleo's Win32-OpenSSH 9.5p2
# had a broken sftp subsystem — `sftp-server.exe` exited immediately on
# invocation, which killed any `scp`/`sftp` INTO cleo. That is fixed as
# of 2026-07-27: a plain push now works, 5/5 hash-verified runs at ~2s
# each, on the same 9.5p2 build. So this is a straight `scp` again, and
# cleo no longer needs SSH access back to the build host. If pushes
# ever start failing that way again, the pull is the workaround — see
# PR #17 for the full diagnostic trail.
#
# Close a running viewport on the target first: Windows locks the .exe of a running process.
viewport-deploy: viewport-build
    #!/usr/bin/env bash
    set -euo pipefail
    exe="viewport/target/x86_64-pc-windows-msvc/release/klams-viewport.exe"
    host="{{viewport_host}}"
    if [[ -z "$host" ]]; then
        echo "viewport-deploy: set VIEWPORT_HOST to the ssh target (user@host) of the Windows machine" >&2
        exit 1
    fi
    dest_dir="$(echo '{{viewport_deploy_dir}}' | tr '\\' '/' | sed 's:/*$::')"
    dest="$dest_dir/klams-viewport.exe"
    echo "→ shipping $exe to $host:$dest"

    scp -q "$exe" "$host:$dest"

    # Verify rather than trust the exit code: a partial write that still
    # exits 0 would otherwise ship a corrupt binary to a machine with no
    # other check on it.
    local_hash=$(sha256sum "$exe" | awk '{print $1}')
    remote_hash=$(ssh "$host" "Get-FileHash '$dest' -Algorithm SHA256 | Select-Object -ExpandProperty Hash" | tr 'A-F' 'a-f' | tr -d '\r')
    if [ "$local_hash" != "$remote_hash" ]; then
        echo "✗ hash mismatch (local $local_hash, remote $remote_hash)" >&2
        exit 1
    fi
    echo "→ deployed and hash-verified. Relaunch klams-viewport.exe on the target to pick up the build."

# Native Linux build of the viewport (webkit2gtk).
# Requires: libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libsoup-3.0-dev.
viewport-build-linux:
    cd viewport && pnpm install --frozen-lockfile && pnpm build
    cd viewport/src-tauri && cargo build --release

# Run the natively-built Linux viewport with devtools enabled.
viewport-run-linux: viewport-build-linux
    ./viewport/target/release/klams-viewport --debug

# sprint-003 T046 — systemd lifecycle helpers.
install-systemd:
    cargo build --release --bin klams-service --bin klams-scanner --bin klams-monitor
    sudo deploy/install-systemd.sh

# Restart the long-running systemd services (service + monitor).
restart:
    sudo systemctl restart klams-service klams-monitor

scanner-once:
    @KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        cargo run --release --bin klams-scanner -- --once

monitor-once:
    @KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        cargo run --release --bin klams-monitor -- --once

# sprint 006 — maintenance + backup operator surface.
# All four are stubs until the sprint phases land them; they exit 1
# so callers fail loudly instead of silently no-op'ing.

backup-once:
    cargo run --quiet -p klams-service -- --run-backup-now

restore-from date *force:
    cargo run --quiet -p klams-service -- --restore-from {{date}} {{force}}

backup-validate-config:
    cargo run --quiet -p klams-service -- --validate-backup-config

# sprint 006 — verify a committed backup pair is readable + intact
# without actually restoring it. Reads `[backup].backup_dir` from
# {{klams_config}} (see the resolution rule at the top).
#
#   just backup-verify                # today's UTC date
#   just backup-verify 2026-05-24     # explicit UTC date
#
# Checks performed:
#   1. Postgres dump: `pg_restore --list` parses the TOC (catches
#      truncation / corruption without writing to a database).
#   2. Qdrant snapshot: magic bytes + `tar tf` listing (qdrant
#      snapshots are uncompressed tar archives).
# Exits 0 iff every check passes; non-zero with the offending step
# on stderr otherwise.
backup-verify date="":
    #!/usr/bin/env bash
    set -euo pipefail
    cfg="{{klams_config}}"
    if [[ ! -r "$cfg" ]]; then echo "backup-verify: $cfg not readable" >&2; exit 1; fi
    backup_dir=$(awk -F '"' '/^[[:space:]]*backup_dir[[:space:]]*=/{print $2; exit}' "$cfg")
    if [[ -z "$backup_dir" ]]; then echo "backup-verify: [backup].backup_dir unset in $cfg" >&2; exit 1; fi
    pg_bin_dir=$(awk -F '"' '/^[[:space:]]*pg_bin_dir[[:space:]]*=/{print $2; exit}' "$cfg")
    pg_restore_bin="${pg_bin_dir:+$pg_bin_dir/}pg_restore"
    d='{{date}}'; if [[ -z "$d" ]]; then d=$(date -u +%Y-%m-%d); fi
    pg_file=$(ls -1 "$backup_dir"/postgres-"$d"*.dump 2>/dev/null | tail -n1 || true)
    q_file=$(ls -1 "$backup_dir"/qdrant-"$d"*.snapshot 2>/dev/null | tail -n1 || true)
    if [[ -z "$pg_file" ]]; then echo "backup-verify: no postgres-$d*.dump in $backup_dir" >&2; exit 1; fi
    if [[ -z "$q_file"  ]]; then echo "backup-verify: no qdrant-$d*.snapshot in $backup_dir" >&2; exit 1; fi
    echo "==> postgres: $pg_file"
    pg_bytes=$(stat -c %s "$pg_file")
    toc=$("$pg_restore_bin" --list "$pg_file" 2>&1)
    pg_entries=$(printf '%s\n' "$toc" | grep -cE '^[0-9]+;' || true)
    if [[ "$pg_entries" -lt 1 ]]; then
        echo "backup-verify: pg_restore --list produced no TOC entries:" >&2
        printf '%s\n' "$toc" >&2
        exit 1
    fi
    printf '  bytes=%s toc_entries=%s OK\n' "$pg_bytes" "$pg_entries"
    echo "==> qdrant:   $q_file"
    q_bytes=$(stat -c %s "$q_file")
    q_members=$(tar tf "$q_file" 2>/dev/null | wc -l)
    if [[ "$q_members" -lt 1 ]]; then
        echo "backup-verify: tar listing produced no members; snapshot may be truncated" >&2
        exit 1
    fi
    printf '  bytes=%s tar_members=%s OK\n' "$q_bytes" "$q_members"
    echo "==> backup-verify: OK"

# sprint 006 T015 — load the scale fixture, run one backup, print
# `kind | bytes | seconds` and append a dated entry to sizing.md.
# Requires the docker-compose.test.yml stack to be running and a
# config file at {{klams_config}} (see the resolution rule at the top).
backup-size:
    @set -eu; \
    cfg="{{klams_config}}"; \
    if [[ ! -r "$$cfg" ]]; then echo "backup-size: $$cfg not readable" >&2; exit 1; fi; \
    backup_dir=$$(awk -F '"' '/^[[:space:]]*backup_dir[[:space:]]*=/{print $$2; exit}' "$$cfg"); \
    if [[ -z "$$backup_dir" ]]; then echo "backup-size: [backup].backup_dir unset in $$cfg" >&2; exit 1; fi; \
    echo "==> loading scale fixture (~10k facts / ~20k knowledge / ~50k events) ..."; \
    cargo test --quiet -p klams-service --features scale-fixture \
        --test scale_loader -- --ignored --nocapture; \
    echo "==> running backup-once ..."; \
    started=$$(date +%s); \
    cargo run --quiet -p klams-service -- --run-backup-now; \
    elapsed=$$(( $$(date +%s) - started )); \
    today=$$(date -u +%Y-%m-%d); \
    pg_file=$$(ls -1 "$$backup_dir"/postgres-$$today*.dump 2>/dev/null | tail -n1); \
    q_file=$$(ls -1 "$$backup_dir"/qdrant-$$today*.snapshot 2>/dev/null | tail -n1); \
    pg_bytes=$${pg_file:+$$(stat -c %s "$$pg_file")}; \
    q_bytes=$${q_file:+$$(stat -c %s "$$q_file")}; \
    printf '\nkind     | bytes        | seconds\n'; \
    printf '%s\n' '---------|--------------|--------'; \
    printf 'postgres | %12s | %s\n' "$${pg_bytes:-NA}" "$$elapsed"; \
    printf 'qdrant   | %12s | %s\n' "$${q_bytes:-NA}" "$$elapsed"; \
    sizing="sprints/006-maintenance-and-backups/sizing.md"; \
    if [[ ! -f "$$sizing" ]]; then \
        printf '# Sprint 006 \xe2\x80\x94 backup sizing log\n\nGenerated by `just backup-size`. Each entry is one `run_once` against the scale fixture (`FixtureScale::large()`: ~10k facts / ~20k knowledge chunks / ~50k events).\n\n' > "$$sizing"; \
    fi; \
    { \
        printf '\n## %s\n\n' "$$(date -u +%Y-%m-%dT%H:%M:%SZ)"; \
        printf '| kind | bytes | seconds |\n|------|-------|---------|\n'; \
        printf '| postgres | %s | %s |\n' "$${pg_bytes:-NA}" "$$elapsed"; \
        printf '| qdrant | %s | %s |\n' "$${q_bytes:-NA}" "$$elapsed"; \
    } >> "$$sizing"; \
    echo "appended entry to $$sizing"

# Atomic rollback: swap *.prev back into place for each klams binary,
# then restart the live services. No-op when no .prev exists.
rollback:
    set -eu; for bin in klams-service klams-scanner klams-monitor; do \
        if [[ -f /usr/local/bin/$bin.prev ]]; then \
            sudo mv -f /usr/local/bin/$bin /usr/local/bin/$bin.broken; \
            sudo mv -f /usr/local/bin/$bin.prev /usr/local/bin/$bin; \
            echo "rolled back $bin"; \
        else \
            echo "no .prev for $bin, skipping"; \
        fi; \
    done; \
    sudo systemctl restart klams-service klams-monitor || true

# sprint 007 — MCP convenience.
# Invoke an MCP tool over Streamable HTTP. Performs the full rmcp
# stateful handshake (initialize -> notifications/initialized ->
# tools/call) and prints the tool's text result. Arguments are passed
# as a raw JSON object. Override KLAMS_URL / KLAMS_TOKEN for a
# non-local stack.
#
# Example:
#   KLAMS_TOKEN=$tok just mcp-call memory_search '{"query":"build"}'
mcp-call tool args='{}':
    #!/usr/bin/env bash
    set -euo pipefail
    url='{{klams_url}}/mcp'
    auth='Authorization: Bearer {{klams_token}}'
    accept='Accept: application/json, text/event-stream'
    ctype='Content-Type: application/json'
    headers=$(mktemp); trap 'rm -f "$headers"' EXIT
    # 1. initialize — capture the mcp-session-id response header.
    curl -sS -D "$headers" -o /dev/null -X POST "$url" \
        -H "$auth" -H "$ctype" -H "$accept" \
        --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"just-mcp-call","version":"0"}}}'
    sid=$(awk 'tolower($1)=="mcp-session-id:" {print $2}' "$headers" | tr -d '\r\n')
    if [[ -z "$sid" ]]; then
        echo "mcp-call: server did not return mcp-session-id" >&2
        cat "$headers" >&2
        exit 1
    fi
    # 2. notifications/initialized (no id, no response body expected).
    curl -sS -o /dev/null -X POST "$url" \
        -H "$auth" -H "$ctype" -H "$accept" -H "mcp-session-id: $sid" \
        --data '{"jsonrpc":"2.0","method":"notifications/initialized"}'
    # 3. tools/call — stream SSE response and decode the result envelope.
    printf '%s' '{{args}}' \
        | jq -c --arg name '{{tool}}' \
            '{jsonrpc:"2.0",id:2,method:"tools/call",params:{name:$name,arguments:.}}' \
        | curl -sS -X POST "$url" \
            -H "$auth" -H "$ctype" -H "$accept" -H "mcp-session-id: $sid" \
            --data @- \
        | sed -n 's/^data: //p' \
        | jq -r 'select(.) | .result.content[0].text // .result // .error // .'

# Copy agent memory primers into the local VS Code (stable) Copilot
# memory-tool directory. Files are taken from .github/memories/ in
# this repo. Safe to re-run — overwrites existing files of the same
# name without touching unrelated memories.
prime-vscode:
    #!/usr/bin/env bash
    set -euo pipefail
    src="{{justfile_directory()}}/.github/memories"
    dest="$HOME/.vscode-server/data/User/globalStorage/github.copilot-chat/memory-tool/memories"
    if [[ ! -d "$src" ]]; then echo "no .github/memories/ directory" >&2; exit 1; fi
    mkdir -p "$dest"
    for f in "$src"/*.md; do
        [[ "$(basename "$f")" == "README.md" ]] && continue
        echo "→ $dest/$(basename "$f")"
        cp -f "$f" "$dest/"
    done

# Same as prime-vscode but targets VS Code Insiders.
prime-vscode-insiders:
    #!/usr/bin/env bash
    set -euo pipefail
    src="{{justfile_directory()}}/.github/memories"
    dest="$HOME/.vscode-server-insiders/data/User/globalStorage/github.copilot-chat/memory-tool/memories"
    if [[ ! -d "$src" ]]; then echo "no .github/memories/ directory" >&2; exit 1; fi
    mkdir -p "$dest"
    for f in "$src"/*.md; do
        [[ "$(basename "$f")" == "README.md" ]] && continue
        echo "→ $dest/$(basename "$f")"
        cp -f "$f" "$dest/"
    done

# sprint 008 — perf fixture seed (FR-019). Always exits 0 (FR-022).
bench-seed *ARGS:
    cargo run --release -p klams-bench --bin seed -- {{ARGS}} || true

# sprint 008 — perf harness run (FR-021). Always exits 0 (FR-022) so
# `just gate` never trips on measurement output.
bench-run *ARGS:
    cargo run --release -p klams-bench --bin run -- {{ARGS}} || true

# sprint 009 — purge every row written by the `klams-bench` agent
# (FR-011 attribution). Resolves the author_id by agent_name, then
# DELETEs from facts / events in Postgres (knowledge lives only in
# Qdrant) and from the Qdrant collection by `author_id` payload
# filter. No payload-pattern fallback. Reads connection settings
# from env:
#   PGPASSWORD, PGHOST=127.0.0.1, PGUSER=klams, PGDATABASE=klams,
#   QDRANT_URL=http://127.0.0.1:6333,
#   QDRANT_COLLECTION=knowledge_items_v2.
bench-clean:
    #!/usr/bin/env bash
    set -euo pipefail
    PGHOST="${PGHOST:-127.0.0.1}"
    PGUSER="${PGUSER:-klams}"
    PGDATABASE="${PGDATABASE:-klams}"
    QDRANT_URL="${QDRANT_URL:-http://127.0.0.1:6333}"
    QDRANT_COLLECTION="${QDRANT_COLLECTION:-knowledge_items_v2}"
    : "${PGPASSWORD:?PGPASSWORD must be set}"
    export PGPASSWORD PGHOST PGUSER PGDATABASE
    author_id=$(psql -h "$PGHOST" -U "$PGUSER" -d "$PGDATABASE" -At \
        -c "SELECT id FROM authors WHERE agent_name = 'klams-bench' ORDER BY last_seen_at DESC LIMIT 1;")
    if [[ -z "$author_id" ]]; then
        echo "bench-clean: no author row for agent_name='klams-bench'; nothing to purge" >&2
        exit 0
    fi
    echo "→ purging rows authored by klams-bench (author_id=$author_id) from $PGHOST/$PGDATABASE"
    psql -h "$PGHOST" -U "$PGUSER" -d "$PGDATABASE" -At -v ON_ERROR_STOP=1 -v aid="$author_id" <<'SQL'
    BEGIN;
    DELETE FROM facts  WHERE author_id = :'aid'::uuid;
    DELETE FROM events WHERE author_id = :'aid'::uuid;
    COMMIT;
    SQL
    echo "→ deleting bench knowledge points from $QDRANT_URL/$QDRANT_COLLECTION"
    curl -sS -X POST "$QDRANT_URL/collections/$QDRANT_COLLECTION/points/delete?wait=true" \
        -H "Content-Type: application/json" \
        -d "{\"filter\":{\"must\":[{\"key\":\"author_id\",\"match\":{\"value\":\"$author_id\"}}]}}" \
        | python3 -c "import sys,json;d=json.load(sys.stdin);print('  qdrant:',d.get('status','?'))"
