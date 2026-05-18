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
