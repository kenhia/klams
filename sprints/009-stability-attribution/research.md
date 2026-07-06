# Phase 0 — Research

## R1. Connection lifecycle tuning (Story 1, FR-001…003)

**Decision**: Apply two HTTP/1 connection-level limits in the
axum/hyper stack — a header read timeout (initial bytes after accept)
and a keep-alive idle timeout (between requests on a kept-alive
connection) — plus a `tower::limit::concurrency::ConcurrencyLimitLayer`
guard at the per-peer level (peer IP via a small custom service that
buckets active connections by remote address).

**Rationale**: The kwi #26 incident showed two distinct fd leaks: (a)
loopback peers that vanished mid-conversation, leaving the server-side
half-open in `CLOSE_WAIT` until the process exited; (b) a
single-peer ESTABLISHED storm (1011 sockets from `kwork`'s viewport
dev loop). Header-read + keep-alive timeouts solve (a) by giving the
server permission to reap. A per-peer concurrency cap solves (b) by
refusing additional accepts above a configurable budget. Both are
standard tower/hyper patterns and require no new crates.

**Defaults**:

- `header_read_timeout`: 30s. Conservative; anything longer than this
  is a misconfigured client.
- `keep_alive_timeout`: 75s. Matches common reverse-proxy defaults
  (nginx, ALB) so kept-alive connections from real clients survive
  but vanished peers are reaped within ~75s.
- `per_peer_max_concurrent`: 64. Two orders of magnitude above any
  legitimate per-peer load we've observed, two orders of magnitude
  below the 1024 fd ceiling.

All three are configurable via a new `[service.limits]` TOML
section (see `contracts/connection-limits.md`).

**Alternatives considered**:

- **Kernel TCP keepalive tuning** (`/proc/sys/net/ipv4/tcp_keepalive_*`).
  Rejected: host-wide, requires root, and only helps the ESTABLISHED
  case — does nothing for CLOSE_WAIT half-opens.
- **A custom `tokio::time::timeout` wrapper around every handler**.
  Rejected: too coarse — would cancel mid-request, not at connection
  idle.
- **Per-route concurrency caps**. Rejected: misses the real problem
  (connection lifecycle), and we have no per-route hotspot.

## R2. Author resolution at service startup (Story 2, FR-007)

**Decision**: Resolve `TokenGrantConfig.agent_name` → `author_id`
eagerly at service startup. Cache the mapping in an
`Arc<HashMap<TokenBytes, AuthorBinding>>` constructed once and
injected as an axum request extension by the auth middleware.

**Rationale**: Eager resolution gives us "fail loud" on misconfigured
agent names (FR-012). It also moves the database round-trip out of
the request hot path — every REST write would otherwise need a
synchronous author lookup. The map is small (token count ≤ 10 in any
realistic deployment) so memory cost is negligible.

**Resolution algorithm**:

```
for each TokenGrantConfig grant in config.auth.tokens:
    let name = grant.agent_name.unwrap_or("system")
    validate(name) or fail-fast with AuthConfigError::InvalidAgentName
    let author = match store.get_author_by_name(name).await:
        Some(author) -> author
        None -> store.register_author(RegisterAuthorArgs{ agent_name: name, ... }).await
    map.insert(grant.token_bytes, AuthorBinding{ author_id: author.id, name })
```

**Alternatives considered**:

- **Lazy resolution** on first write per token. Rejected: misconfiguration
  surfaces only when a write happens, possibly hours/days after
  startup.
- **Resolution inside the store layer** (each write does its own
  lookup). Rejected: hot-path cost and no place for the validation
  failure to live.

## R3. Pipeline carrier shape (Story 2, FR-008)

**Decision**: Add `author_id: Uuid` to `UpsertFact`, `AppendEvent`,
and `IndexKnowledge` as a required field. Update all construction
sites (handlers + tests).

**Rationale**: Making the field required at the type level prevents
the very class of bug we're fixing — it becomes impossible to
construct a `MemoryWrite` job without an author. The compiler enforces
that every handler thinks about attribution.

**Alternatives considered**:

- **`Option<Uuid>` with a `SYSTEM_AUTHOR_ID` default**. Rejected:
  silently re-enables the current bug if a handler forgets to set it.
- **Pass author alongside `MemoryWrite` in a tuple at worker
  dispatch**. Rejected: same forgetability risk; also splits the
  data shape across two locations.

## R4. Re-attribution algorithm (Story 3, FR-013…016)

**Decision**: For each `system`-stamped row, look up the most recent
event in the `events` table referencing the row's id with a
non-system `author_id`. If exactly one non-system author is implicated
across that row's provenance, reassign; otherwise leave under
`system` and log to the "unrecoverable" bucket.

**Rationale**: The existing `events` table records per-write
metadata, and sprint 007 already attributes events via the
`_with_author` path on the MCP surface — that gives us a partial but
high-confidence trail. For sprint 008 we also added `event_search` —
which means events for REST writes were *also* being created, just
that the corresponding `facts` row got `system`. Cross-reference is
straightforward.

**Provenance signal — concrete shape**:

- For each row in `facts` where `author_id = SYSTEM_AUTHOR_ID`:
  - look up `events` where `payload->>'fact_id' = facts.id::text`
    and `author_id <> SYSTEM_AUTHOR_ID` ordered by `created_at`.
  - if all such events share a single `author_id` and that author
    row still exists in `authors`, that's the attribution — bucket
    as `reassigned_to_recovered_author`.
  - if 0 events match, bucket as `reassigned_to_lost_author`
    (write the row's `author_id` to `LOST_AUTHOR_ID`).
  - if >1 distinct non-system authors implicated, bucket as
    `reassigned_to_lost_author`.
  - if the recovered author no longer exists in `authors`, bucket
    as `reassigned_to_lost_author`.
  - if no `system`-distinguishing provenance exists at all (no
    matching events of any kind), bucket as `left_as_system` and
    leave the row untouched.
- Mirror for `events` and `knowledge_items` with their respective
  id key.

**Idempotency**: The repair only updates rows where
`author_id = SYSTEM_AUTHOR_ID`. Once a row is reassigned (to a
recovered author or to `lost-author`), a second run finds it under
its new author and skips it. Reruns therefore report zero changes
— satisfies FR-014.

**Alternatives considered**:

- **Use the bearer-token audit log** (if we had one). Rejected: no
  such log exists today, and creating one retroactively is impossible.
- **Heuristic on `source` enum** (`Controller` → `controller` author).
  Rejected: too lossy; `source` is a class label, not an identity.

## R5. Qdrant payload stamping (Story 2, FR-008 for knowledge)

**Decision**: Add `author_id` (lowercase-hyphenated UUID string) and
`author_agent_name` (string) to every payload written by
`index_knowledge_with_author`. The current `index_knowledge` (no
author) is deleted — every call site moves to the `_with_author`
variant.

**Rationale**: Knowledge items live only in Qdrant — there's no
Postgres-side author column. Stamping the payload is the only way to
filter by author later (the existing `event_search` MCP tool already
uses Postgres for events; knowledge needs the payload route).

**Backfill**: The Story 3 repair walks `knowledge_items` payloads
filtered by `author_id = SYSTEM_AUTHOR_ID` and rewrites each point's
payload via Qdrant's payload-update API.

**Alternatives considered**:

- **Mirror knowledge into a Postgres index table for attribution
  filtering**. Rejected: duplicates state, needs its own consistency
  story.

## R6. Phase 6 test isolation (Story 6, FR-021)

**Decision**: Each Phase 6 test creates its own ephemeral Qdrant
collection named `klams_test_{Uuid::new_v4()}` and truncates the
shared facts/events tables in test setup. Drop the collection on
test teardown.

**Rationale**: Cheapest viable isolation. Qdrant collection create +
drop is fast (<100ms in our setup). Postgres truncates of small test
tables are likewise fast.

**Alternatives considered**:

- **testcontainers per test**: 10×+ slower; needs Docker socket access
  in CI; over-engineering for one assertion.
- **Serial-only marker on the offending test**: papers over the bug
  instead of fixing it.

## R7. Viewport href unification (Story 4, FR-017…018)

**Decision**: Export the existing `hrefFor(memory)` from
`viewport/src/routes/activity/row.ts` (already public after sprint
008's T054 extraction) and have the Authors view's memory list
component call it instead of computing its own URL. Add a vitest case
under `viewport/src/routes/authors/[id]/row.test.ts` that imports the
Authors row component and asserts the rendered `<a href>` matches
`hrefFor()` for fact / event / knowledge kinds.

**Rationale**: The bug is route drift: the Authors view ships a
bespoke URL that doesn't match any SvelteKit route. The sprint 008
helper is already the source of truth for memory routing; reuse it.

**Alternatives considered**:

- **Add a missing SvelteKit route to match the bespoke URL**.
  Rejected: leaves two link-builders in the codebase and invites the
  same drift later.
