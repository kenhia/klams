# klams — Local Agent Memory System

`klams` is a self-hosted, personal memory service for AI agents: one
durable, observable place where every agent you run — Claude Code,
GitHub Copilot, anything MCP-capable — reads and writes shared memory,
so what one agent learns another can recall. It grew up in one
specific homelab (the "k" is its author, Ken) and is developed against
that environment, but it installs anywhere Linux, Docker, and a Rust
toolchain are available — see [docs/install.md](docs/install.md).

## What it is

klams stores three kinds of memory behind one HTTP + MCP surface:

- **User memory** — stable facts about you, your machines, and
  preferences.
- **Task memory** — repos, services, sprint state, execution traces,
  events.
- **Knowledge memory** — semantic content scanned from your code and
  notes, plus prose findings agents write back.

Retrieval is hybrid: dense vectors (Qdrant + a text-embeddings
server), Postgres full-text, and a curated stratum, fused with
reciprocal rank fusion and re-ranked by a cross-encoder. Agents talk
to it over MCP (Streamable HTTP) with scoped, attributed bearer
tokens; writes carry their author.

Alongside the service run three companions:

- **klams-scanner** — walks your configured roots and keeps the
  knowledge corpus in sync with your files.
- **klams-monitor** — turns systemd unit state edges into events.
- **viewport** — a Tauri + Svelte desktop app; the human-facing
  window into (and curation surface for) klams state.

## Getting started

**[docs/install.md](docs/install.md)** is the one guide: host
prerequisites, the GPU/CPU/OpenAI-compatible-endpoint decision tree,
provisioning, first tokens, connecting an agent, and the first-run
smoke check (`just smoke`) that proves the install end-to-end.

Then, going deeper:

- [docs/architecture.md](docs/architecture.md) — design and component
  map (diagrams under [docs/diagrams/](docs/diagrams/)).
- [docs/usage.md](docs/usage.md) — day-to-day operator recipes.
- [docs/auth.md](docs/auth.md) — who can do what; how tokens are
  granted.
- [docs/klams-mcp-for-agents.md](docs/klams-mcp-for-agents.md) — hand
  this to an AI agent to wire it up, including the routing-policy
  blurb that makes agents actually use the store.
- [docs/setup.md](docs/setup.md) — the reference deployment's
  operational record.
- [AGENTS.md](AGENTS.md) — the working agreement for humans and
  agents changing this repo.

The elevator pitch — what klams is, why it exists, and what it
demonstrably does, with real dated numbers — is a single
self-contained page: [docs/pitch/klams-pitch.html](docs/pitch/klams-pitch.html)
([rendered preview](https://htmlpreview.github.io/?https://github.com/kenhia/klams/blob/main/docs/pitch/klams-pitch.html)).
It renders offline straight from a checkout too.

## Support & posture

- **Support**: best effort, no SLA. Issues are welcome; PRs are
  better. This is one person's production system that happens to be
  installable — expect honest docs, not a support organization.
- **Hardware**: the embedding backend must be one of NVIDIA CUDA
  (TEI GPU image), CPU (TEI `cpu-*` image), or any OpenAI-compatible
  embeddings endpoint you bring (vLLM, Ollama, a cloud API). Nothing
  else is supported — in particular there is no first-party ROCm or
  Metal path; on those machines, bring an OpenAI-compatible endpoint.
- **Versioning / upgrades**: the version's PATCH segment is a sprint
  number, not a compatibility signal — `0.1.34 → 0.1.35` says "one
  sprint later", nothing more. Postgres migrations are forward-only
  and run automatically at service start. Corpus-shape changes
  (embedding model or collection swaps) have no upgrade path except
  re-scanning, which is cheap by design. The policy, honestly stated:
  **run `main`, expect to re-scan occasionally.** No release tags are
  cut yet; that starts when there's a second operator who needs them.
- **Contributing**: deliberately deferred — no CONTRIBUTING.md, issue
  templates, or PR policy until a real contributor materializes
  (recorded in sprint 035 so it isn't re-litigated). If that's you,
  open an issue and say hi.

## License

[MIT](LICENSE)
