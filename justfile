# klams — one-command developer inner loop.
#
# `just --list`  ─ enumerate recipes
# `just gate`    ─ constitution pre-commit gate (also what CI runs)
# `just verify`  ─ end-to-end functional smoke against a running service

set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Default recipe shows the menu so a bare `just` is friendly.
default:
    @just --list

# Service URL + bearer token used by `verify` and `health`. Override
# in the environment when pointing at a non-local stack.
klams_url     := env_var_or_default('KLAMS_URL',   'http://127.0.0.1:7777')
klams_token   := env_var_or_default('KLAMS_TOKEN', 'dev-token')
compose_file  := 'deploy/docker-compose.yml'

# Bring the Postgres+Qdrant+TEI stack up in the background.
compose-up:
    docker compose -f {{compose_file}} up -d

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

# Workspace-wide tests (excludes #[ignore]'d cases).
test:
    cargo test --workspace

# Constitution pre-commit gate — fail-fast on fmt, clippy, or tests.
# CI invokes exactly this recipe (no inline duplication).
gate:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace

# Quick liveness probe + light verification round-trip.
health:
    KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        bash scripts/verify-mvp.sh --light

# Full SC-001..SC-009 functional smoke (slower than `health`).
verify:
    KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        bash scripts/verify-mvp.sh

# Cross-compile the viewport for Windows (requires cargo-xwin).
viewport-build:
    cd viewport/src-tauri && cargo xwin build --release --target x86_64-pc-windows-msvc

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

scanner-once:
    KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
        cargo run --release --bin klams-scanner -- --once

monitor-once:
    KLAMS_URL={{klams_url}} KLAMS_TOKEN={{klams_token}} \
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

# sprint 006 T015 — load the scale fixture, run one backup, print
# `kind | bytes | seconds` and append a dated entry to sizing.md.
# Requires the docker-compose.test.yml stack to be running and a
# config file at $KLAMS_CONFIG (or /ai/klams/config/klams.toml).
backup-size:
    @set -eu; \
    cfg="$${KLAMS_CONFIG:-/ai/klams/config/klams.toml}"; \
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
    sizing="specs/006-maintenance-and-backups/sizing.md"; \
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
