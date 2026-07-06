# Contracts — Sprint 009

Each file documents a stable interface that the implementation must
match. Tests in the corresponding crate assert these shapes.

| File | Scope |
|------|-------|
| `token-grant-config.md` | TOML shape, startup resolution algorithm, validation errors |
| `connection-limits.md` | `[service.limits]` TOML keys, defaults, log events emitted on reap |
| `reattribution-cli.md` | `reattribute-system` CLI flags, report shape, exit codes, idempotency |

The REST write surfaces (`POST /v1/facts|events|knowledge`) keep
their existing wire contracts from sprint 005 — only the
attribution **semantics** change. That semantic change is documented
inline in `data-model.md` (pipeline structs gain `author_id`) and in
the relevant handlers' contract tests, which assert that a write
authenticated by a token with `agent_name = "alice"` produces a row
whose `author_id` matches alice's id (not `SYSTEM_AUTHOR_ID`).

The MCP surface is unchanged; no contract addition needed.
