# klams — Ken's Local Agent Memory System

> **Disclaimer:** This project is purpose-built for Ken's specific hardware
> and homelab environment (notably the `kubs0` and `kai` machines, a
> particular Postgres/Qdrant/GPU layout, and Ken's controller + GHCP agent
> setup). It is **not** intended as a general-purpose agent memory system.
> Paths, hostnames, services, and assumptions throughout the code and docs
> reflect that environment and will not work elsewhere without
> non-trivial changes.

## What it is

`klams` is a controller-centric, shared memory service for Ken's homelab
agent ecosystem. It provides a unified, durable, and observable place to
read and write three kinds of memory:

- **User memory** — stable facts about Ken, his machines, and preferences.
- **Task memory** — repos, services, sprint state, execution traces, events.
- **Knowledge memory** — semantic content from the Obsidian vault, specs,
  READMEs, and troubleshooting notes.

A companion Tauri + Svelte desktop **viewport** runs on Windows and is the
human-facing window into klams state.

## Status

Initial MVP shipped. The service runs under systemd on `kubs0` and the
Windows viewport reads facts, events, and knowledge over the LAN.

- [specs/001-initial-mvp/spec.md](specs/001-initial-mvp/spec.md) —
  MVP specification + success criteria (SC-001..SC-009).
- [specs/001-initial-mvp/plan.md](specs/001-initial-mvp/plan.md) —
  implementation plan.
- [specs/001-initial-mvp/tasks.md](specs/001-initial-mvp/tasks.md) —
  task ledger.
- [.specify/memory/constitution.md](.specify/memory/constitution.md) —
  project constitution.

## Running the MVP

Full end-to-end provisioning, build, and smoke checks are in the
quickstart. The short version:

1. **Provision `kubs0`**: install Docker + systemd, then run the
   storage-root + Compose bootstrap per
   [docs/setup.md](docs/setup.md).
2. **Bring up dependencies**:

   ```sh
   cd deploy
   docker compose --env-file compose.env up -d
   ```

3. **Run migrations and start the service**:

   ```sh
   cargo run -p klams-service --release   # or: systemctl start klams
   ```

4. **Build the Windows viewport** from any Linux host with
   `cargo-xwin` installed:

   ```sh
   cd viewport/src-tauri
   cargo xwin build --release --target x86_64-pc-windows-msvc \
     --features custom-protocol
   ```

   Copy the resulting `klams-viewport.exe` to the Windows machine, set
   the bearer once via the in-app config dialog (stored in the Windows
   Credential Manager), and launch. Add `--debug` to open WebView
   devtools and enable diagnostic logging.

5. **Smoke-test** against the success criteria using
   [specs/001-initial-mvp/quickstart.md §9](specs/001-initial-mvp/quickstart.md#9-smoke-test-the-user-stories).

### Sprint 002 quick reference

Sprint 002 (`specs/002-safety-and-write-ops/`) adds a top-level
[`justfile`](justfile) so every routine task — bringing the stack up,
running the service, exercising the pre-commit gate, smoke-testing
the API — is a single discoverable recipe and is the same command CI
invokes. Install [`just`](https://github.com/casey/just) once
(`cargo install just`), then:

```sh
$ just --list
Available recipes:
    build           # Release build of the service binary.
    compose-down    # Stop and remove the stack (keeps volumes).
    compose-rebuild # Force a clean rebuild of all compose images.
    compose-up      # Bring the Postgres+Qdrant+TEI stack up in the background.
    default         # Default recipe shows the menu so a bare `just` is friendly.
    gate            # CI invokes exactly this recipe (no inline duplication).
    health          # Quick liveness probe + light verification round-trip.
    run             # Run the service in the foreground; logs go to stderr.
    test            # Workspace-wide tests (excludes #[ignore]'d cases).
    verify          # Full SC-001..SC-009 functional smoke (slower than `health`).
    viewport-build  # Cross-compile the viewport for Windows (requires cargo-xwin).
```

The full sprint 002 walkthrough (validation, dissents, decay,
viewport curation, `just gate`) lives at
[specs/002-safety-and-write-ops/quickstart.md](specs/002-safety-and-write-ops/quickstart.md).

Architecture overview and component map:
[docs/architecture.md](docs/architecture.md). Day-to-day operator
recipes: [docs/usage.md](docs/usage.md).

## License

[MIT](LICENSE)
