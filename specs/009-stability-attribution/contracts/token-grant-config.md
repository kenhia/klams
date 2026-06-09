# Contract — Token Grant Config (`agent_name`)

## TOML shape

```toml
[[auth.tokens]]
token = "abc123..."           # required, hex/base64 secret material
scopes = ["memory:write"]     # required, existing scope enum
note = "alice's laptop"       # optional
agent_name = "alice"          # NEW, optional, lowercase ascii [a-z0-9_-]{2,64}
```

## Validation

Performed at config deserialization (service startup):

| Condition | Outcome |
|-----------|---------|
| `agent_name` absent or `None` | OK; grant resolves to `system` author |
| `agent_name = "alice"` and matches charset/length | OK; resolution proceeds |
| `agent_name = ""` | `ConfigError::InvalidAgentName { reason: "empty" }` |
| `agent_name = "Alice"` (uppercase) | `ConfigError::InvalidAgentName { reason: "charset" }` |
| `agent_name = "a"` (too short) | `ConfigError::InvalidAgentName { reason: "length" }` |
| `agent_name = "<65 chars>"` | `ConfigError::InvalidAgentName { reason: "length" }` |

Service refuses to bind on any validation error.

## Startup resolution

For each `TokenGrantConfig` with a non-None `agent_name`:

1. `store.get_author_by_name(&name)` →
2. If `Some(author)`: bind `token_bytes -> AuthorBinding { author.id, name }`.
3. If `None`: `store.register_author(RegisterAuthorArgs { agent_name: name, .. })`
   then bind to the newly created author.
4. Log `tracing::info!(author_id, agent_name, "token bound to author")`.

For grants with `agent_name = None`, bind to the seeded `system` author
(`SYSTEM_AUTHOR_ID`) without a store roundtrip.

## Request-time wiring

The existing auth middleware identifies the bearer token, then attaches
the `AuthorBinding` cloned from the startup map as an axum request
extension. REST handlers extract via:

```rust
async fn create_fact(
    Extension(binding): Extension<AuthorBinding>,
    Json(body): Json<CreateFactRequest>,
    ...
) -> Result<...> {
    let job = UpsertFact { author_id: binding.author_id, ... };
}
```

## Contract tests

Located in `crates/klams-service/tests/auth_attribution.rs`:

- T1: token with `agent_name = "alice"` causes `POST /v1/facts` to
  produce a row with `facts.author_id = alice.id`.
- T2: token without `agent_name` produces `facts.author_id = SYSTEM_AUTHOR_ID`.
- T3: config with `agent_name = "ALICE"` causes startup failure with
  `ConfigError::InvalidAgentName { reason: "charset" }`.
- T4: same `agent_name` on two different tokens binds both to the same
  `author_id`.
