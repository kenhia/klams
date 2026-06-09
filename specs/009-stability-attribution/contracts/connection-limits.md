# Contract — Connection Limits (`[service.limits]`)

## TOML shape

```toml
[service.limits]
header_read_timeout_secs = 30   # OPTIONAL, default 30
keep_alive_timeout_secs  = 75   # OPTIONAL, default 75
per_peer_max_concurrent  = 64   # OPTIONAL, default 64
```

All keys are optional. Missing section ⇒ all defaults applied.

## Validation

| Key | Range | On violation |
|-----|-------|--------------|
| `header_read_timeout_secs` | `1..=600` | `ConfigError::InvalidLimit` at startup |
| `keep_alive_timeout_secs` | `1..=3600` | `ConfigError::InvalidLimit` at startup |
| `per_peer_max_concurrent` | `1..=10_000` | `ConfigError::InvalidLimit` at startup |

## Behavior

### Header read timeout

Wraps the hyper `Http1Builder::header_read_timeout`. If no full
request headers arrive within the window, the server closes the
connection and emits:

```text
{"level":"info","target":"klams_service::limits",
 "event":"connection.header_read_timeout","peer":"...","elapsed_ms":...}
```

### Keep-alive timeout

Wraps `Http1Builder::keep_alive(true)` plus a tower
`IdleTimeoutLayer` of the configured duration on each accepted
connection. Idle connections beyond the window are closed and emit:

```text
{"level":"info","target":"klams_service::limits",
 "event":"connection.keep_alive_timeout","peer":"...","elapsed_ms":...}
```

### Per-peer concurrency cap

A small `tower::Service` wrapper around the listener buckets active
connections by remote IP. When a peer's bucket would exceed
`per_peer_max_concurrent`, the new connection is accepted-then-closed
immediately (TCP RST + log entry). No request is parsed.

```text
{"level":"warn","target":"klams_service::limits",
 "event":"connection.per_peer_cap_exceeded","peer":"...","active":...}
```

The cap is hard — there is no queue.

## Contract tests

Located in `crates/klams-service/tests/connection_limits.rs`:

- T1: client that opens a TCP connection and never sends headers is
  closed within `header_read_timeout_secs + 5s`.
- T2: client that completes one request and goes silent has its
  connection closed within `keep_alive_timeout_secs + 5s`.
- T3: 100 simultaneous connections from the same peer with
  `per_peer_max_concurrent = 8` see ≥ 92 immediately closed; the
  remaining ≤ 8 successfully serve a request.
- T4 (smoke): `tools/soak` running for 10 minutes against a 64-fd
  budget keeps the fd count bounded (see SC-001 for the long-window
  variant).
