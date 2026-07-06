# MCP Tool Contracts — sprint 007

This directory holds the public contracts the MCP server exposes to
external agents. Each tool has a JSON Schema for its input arguments.
Output shapes are documented inline in [tools.md](./tools.md) and
mirror the `PublicMemory` projection defined in
[../data-model.md §6](../data-model.md#6-memory-projection-over-the-wire).

REST endpoints added in this sprint (for viewport author drilldown)
are documented in [rest-authors.md](./rest-authors.md).

Files:

- [`tools.md`](./tools.md) — human-readable tool reference and outputs.
- [`tool-schemas/`](./tool-schemas/) — one JSON Schema per tool input.
- [`rest-authors.md`](./rest-authors.md) — new REST endpoints `GET /v1/authors`, `GET /v1/authors/{id}`, `GET /v1/authors/{id}/memories`.
- [`error-codes.md`](./error-codes.md) — canonical MCP error codes returned via `_meta.error_code`.
