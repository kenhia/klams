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

Planning. See:

- [specs/planning/plan.md](specs/planning/plan.md) — phased roadmap
- [specs/planning/viewport.md](specs/planning/viewport.md) — desktop GUI plan
- [specs/planning/backlog.md](specs/planning/backlog.md) — deferred ideas
- [.specify/memory/constitution.md](.specify/memory/constitution.md) —
  project constitution

## License

[MIT](LICENSE)
