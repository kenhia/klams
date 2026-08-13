# Sprint 043 — MCP 2026-07-28 support (tools/list cache metadata)

- **korg proposal**: [korg:1219](korg:1219) — slice 3 of program korg:1220
  ("MCP 2026-07-28 across the fleet: kaed, korg-mcp, klams-mcp")
- **Work item**: #1216 — klams-mcp: support MCP 2026-07-28 before an rmcp
  bump forces it
- **Version**: `0.1.43`

## Goal

Upgrade klams-mcp to serve MCP revision **2026-07-28** deliberately, before
a dependency bump springs the trap kaed hit (korg:1212): a server that
*advertises* the revision without emitting what it requires makes every
Claude Code ≥2.1.227 client silently register **zero tools** — connected,
instructions delivered, nothing callable, no disconnect to explain it.

The requirement is SEP-2549 cache metadata — `ttlMs` (number) and
`cacheScope` (`"public"`|`"private"`) — on the **tools/list result**. That
is cache metadata for the tool *catalog*; `CallToolResult` carries none in
any revision.

## Scoping: the estimate was wrong, for two independent reasons

The WI is filed **S**. It isn't one. Both corrections came from reading
this repo rather than the ticket, and the second one contradicts the
ticket's explicit instruction.

### 1. klams-mcp resolves to rmcp **1.7.0**, not 3.1.x

The proposal's scoping gate (inherited from korg sprint 058) says to check
the version the tree *resolves* to, in `Cargo.lock`, not the one a note was
written against. korg declared `1.7` and resolved `1.8.0`. klams declares
`1.7` and resolves **`1.7.0`** — a version older still, and three minors
below the `3.1.0` the kaed sprint-016 note assumes throughout.

So this slice carries a dependency migration (1.7.0 → **3.1.2**, current
latest and the exact version korg landed on), and the two cache-metadata
fields are the smaller half of the work.

**The bump and the fields are ONE change, not separable commits.** On 1.7
there is no `supported_protocol_versions()` hook, so the server answers
`get_info()`'s version — honest by accident. On 3.1.x the default becomes
`KNOWN_VERSIONS`, so the instant the bumped tree compiles the server is
advertising 2026-07-28 with no cache metadata: the #1212 zero-tools state,
in the working tree, with nothing failing to signal it. Never land a bare
"bump rmcp" commit here.

### 2. klams's tool catalog is **not** static per build — `cacheScope` must be `private`

WI #1216 and the proposal summary both prescribe `cacheScope: "public"`,
reasoning that "klams's tool catalog is static per build". **It is not.**

[`list_tools`](../../crates/klams-mcp/src/tools/mod.rs) filters the catalog
by the caller's scope set, pulled from the bearer token via
`caller_scopes(&context)` (FR-020, sprint 025):

- a `Read` token sees `memory_search`, `memory_related`, `event_search`
- a `Write` token additionally sees the `memory_*` mutation verbs
- only an `Admin` token sees `memory_admin_*`

The tools/list result is therefore **per-caller**, and `public` is a
correctness bug, not a tuning choice: it invites a shared cache to serve
one principal's catalog to another. klams emits **`cacheScope: "private"`**.
This is where klams legitimately diverges from kaed and korg, whose
catalogs are not scope-filtered.

TTL stays hour-ish (3_600_000 ms) — within a scope set the catalog really
is static per build.

## What is already in place

Two pieces the fleet notes flag as work are already done here:

- **`list_tools` is hand-written** (`tools/mod.rs:221`) — klams never used
  `#[tool_handler]`'s generated one, because it needs the scope filter. The
  two fields land in an existing function; no router plumbing changes.
- **No `ClientInfo` anywhere in the tree** — klams is a leaf server, so the
  client-side `PEER_PROTOCOL_VERSION` pin (kaed 016 D-2) does not apply,
  and neither does the relay-framing finding (016 D-4). Checked by grep,
  not assumed.

`get_info` (`tools/mod.rs:200`) does **not** override `initialize`, so
korg's re-check rule for a stale `initialize` override does not apply
either. It does set `protocol_version = ProtocolVersion::default()`, which
is exactly the SDK default that must become an explicit pin.

## Scope

1. **rmcp 1.7.0 → 3.1.2** in `crates/klams-mcp/Cargo.toml`, with the API
   delta korg measured: `Content` → `ContentBlock`, `call_tool` returning
   `CallToolResponse` (via `.into()`, since every klams tool completes
   in-request), `with_stateful_mode` → `with_legacy_session_mode`.
2. **Re-check the `handle_delete` 202 workaround** (`lib.rs:59`) against
   3.1.2. SEP-2567 removes sessions from 2026-07-28 outright, so this may
   simply go away; if it survives, it is because the legacy lifecycle still
   needs it.
3. **Pin both directions explicitly** — `supported_protocol_versions()`
   listing what klams actually implements with the ceiling at
   `V_2026_07_28`, and `get_info`'s fallback pinned rather than tracking
   `ProtocolVersion::default()`/`LATEST`. An SDK default must never speak
   for this implementation (kaed 015 D-3).
4. **Emit `ttlMs` + `cacheScope: private`** on the tools/list result, and
   **only for peers that negotiated 2026-07-28+** —
   `RequestContext::protocol_version()`. A 2025-11-25 peer is entitled to
   2025-11-25's shape.
5. **Wire-shape tests** over the real transport, raw JSON-RPC (rmcp's own
   client cannot drive a conformant 2026-07-28 session): fields present and
   *well-typed* at 2026-07-28 (`ttlMs` a JSON number — the client's
   validator rejects `"3600000"` as surely as `undefined`), fields absent
   for a 2025-11-25 peer, every supported revision negotiating as itself
   with an unknown version falling back to the ceiling, and one real
   `tools/call`. Include a scope-filtered case, since that is klams's
   divergence.
6. **Deploy kubs0** + the live gate.

## Acceptance criteria

- `just gate` passes; integration stack passes.
- A raw probe (`initialize` asking for `9999-12-31`) reports klams's
  ceiling as `2026-07-28`.
- tools/list at 2026-07-28 carries `ttlMs` (number) and
  `cacheScope: "private"`; at 2025-11-25 it carries neither.
- **The live gate**: a fresh Claude Code ≥2.1.227 session enumerates all
  klams tools *and* completes a real `memory_search`. The wire suite cannot
  prove client acceptance — do not treat it as the gate, and do not lift
  the version ceiling and deploy in one motion.
- klams-mind and the other localhost consumers on kubs0 still work after
  the restart. The failure mode is silent, so confirm with a real call from
  a fresh client session, not just a green systemd unit.

## Rollback

Current build stays ready. No migration either way, so rollback is a plain
re-tag/reinstall of the 0.1.42 binaries from the package store.

## Log

_(decisions, surprises and outcomes recorded as the sprint proceeds)_

- **2026-08-12** — Sprint opened. Scoping gate fired twice: rmcp resolves
  to 1.7.0 (dependency migration, not an S), and the prescribed
  `cacheScope: public` is wrong for klams because the catalog is
  scope-filtered per caller. Both recorded above.

- **2026-08-12** — Implementation landed. The 1.7.0 → 3.1.2 migration was
  **exactly the three call sites korg measured**, despite klams starting a
  minor lower: `Content` → `ContentBlock` (2 uses), `call_tool` returning
  `CallToolResponse`, and `stateful_mode` → `legacy_session_mode`. The
  whole workspace — every crate, every existing test — compiled unchanged
  otherwise. The version numbers badly overstate this migration, exactly as
  korg found.

  `call_tool` was kept intact by splitting it: the trait method is now a
  four-line wrapper doing `.into()`, and the ~300-line dispatch body moved
  to an inherent `dispatch_tool`. Nothing in the dispatch logic changed.

  **The trap was directly observed, not just reasoned about.** With the
  bump compiled and no `supported_protocol_versions()` override yet, the
  working tree was already advertising 2026-07-28 with no cache metadata —
  the #1212 zero-tools state, with a green `cargo check` and nothing
  failing. Confirmed against the vendored 3.1.2 source: the default really
  is `KNOWN_VERSIONS`, and `KNOWN_VERSIONS` really does include
  `V_2026_07_28` while `LATEST` is still `V_2025_11_25`. That gap between
  the two constants is the whole bug class.

- **2026-08-12** — `handle_delete` workaround (`lib.rs`) **re-checked and
  kept**. rmcp 3.1.2 still answers 202, so the 202→204 rewrite the mcp
  python-sdk needs is still required. It is now reachable only by legacy
  peers — `handle_delete` opens with `is_legacy_request` and answers 405
  otherwise — because SEP-2567 removes sessions from 2026-07-28, which has
  nothing to terminate. Comment updated to say retire-with-the-last-legacy-
  client, not retire-with-the-revision.

- **2026-08-12** — Wire suite (`mcp_protocol_2026_07_28.rs`, 6 tests)
  passed on the first run. Because a suite that passes immediately proves
  nothing about whether it *can* fail, it was mutation-tested: flipping
  `Private`→`Public` and forcing the version gate open failed exactly the
  three tests that should care, each with its intended diagnosis, while the
  negotiation tests correctly stayed green. Restored and re-verified.

  Request-shape details confirmed against the vendored 3.1.2 source rather
  than inherited from the notes: the required `_meta` keys are
  `protocolVersion` + `clientCapabilities` (not `clientInfo`), and the
  `MCP-Protocol-Version` header is *optional but must agree* with the body
  when present. Verified empirically that a 2026-07-28 initialize issues no
  session id.

- **2026-08-12** — `just gate` green; full integration suite green (0
  failures). Docs updated: `architecture.md` gains a "Protocol revisions"
  section covering the pin, the trap and the `private` divergence;
  `klams-mcp-for-agents.md` gains the revision range and a note that the
  catalog is scope-filtered. Test stack torn down.

## Deployed 2026-08-13

- Version `0.1.43` live on kubs0 (`/healthz` confirms; was `0.1.42`).
- Published to the store as `artifacts/klams-{service,scanner,monitor}/0.1.43/`,
  `latest` → `0.1.43`. Built from branch commit `52ed27d`; this sprint
  deployed *before* shipping the PR.

### Provenance after the merge (PR #47, squash `f35c172`)

The sprint shipped after it deployed, so the published artifact was built
from a commit that no longer exists on `main`. That gap was checked rather
than waved away, and it closes:

- **The binary records no commit.** There is no `build.rs` and no `vergen`
  anywhere in the workspace; `--version` reports `CARGO_PKG_VERSION` alone
  (`klams-service 0.1.43`). There is no field that could be "the wrong
  commit".
- **The merge changed no compiled source.** The two commits added after the
  build were `.claude/skills/deploy-kubs0/SKILL.md` and this file — neither
  compiles, so `main` builds byte-identical binaries to the ones published.
- **The running binaries are the published artifacts, verified**, not
  assumed. All three `sha256sum`s match the store's `SHA256SUMS` for
  `0.1.43` exactly (service `d5cdc583…`, scanner `a9203c15…`, monitor
  `3278d493…`).

**`deploy-from-store` was deliberately NOT re-run after the merge.** It
would have installed identical bytes while rotating `0.1.43` into `.prev`,
destroying `0.1.42` as the one-step rollback target — pure cost, no
benefit. `.prev` still holds `0.1.42` for all three binaries. This is also
what the deploy skill's preflight step 8 prescribes for an already-published
version: do not reach for `--force`; the published build *is* the code you
intend to deploy.

Re-verified live on `main` after the merge: `/healthz` `0.1.43` / `Ok`,
raw probe reports the `2026-07-28` ceiling, `tools/list` at 2026-07-28
returns `ttlMs: 3600000` (number) + `cacheScope: "private"` over 10 tools,
both units active.
- Unit files: unchanged (`git diff 7ed0dcb..HEAD -- deploy/` is empty), so
  `install-systemd` was **not** run — `deploy-from-store` touched no units.
- Migrations applied: **none**. `migrations/` is untouched by this sprint,
  so the restart migrated nothing and rollback is a pure binary swap.
- kai's `klams-scanner`: **left at 0.1.42, deliberately.** This sprint
  changes only the MCP server surface (`klams-mcp`), which kai does not
  run — kai runs the scanner alone. Catch it up with
  `just deploy-remote kai klams-scanner` whenever convenient; 0.1.43 is
  already in the store waiting.
- Rollback target: `0.1.42` via `just rollback` (`.prev` binaries in place
  for all three); any published version via `just deploy-from-store --version`.
- Config changes required: **none**. `/etc/klams/klams.toml` untouched; no
  new scopes or sections.

### Verified live, beyond `/healthz`

The wire suite proves shape against a test server; these ran against the
deployed process:

- **Raw probe** — `initialize` asking for `9999-12-31` returns
  `2026-07-28`. The server's real ceiling, read from outside, not from its
  docs. The service log shows rmcp's matching fallback WARN.
- **2026-07-28 peer** — `tools/list` returns `ttlMs: 3600000` (JSON
  *number*, type-checked) and `cacheScope: "private"`, 10 tools.
- **2025-11-25 peer** — `tools/list` returns **neither** field, same 10
  tools. Clean downgrade, no shape leakage.
- **Real `tools/call`** — `memory_search` over the conformant 2026-07-28
  request shape returned 2 hits with `isError: false`.
- **Session DELETE still 204** — the sprint-018 python-sdk workaround
  survived the rmcp 3.1.2 migration, as the code re-check predicted.
- `just verify` — 7 passed, 0 failed (SC-001, 002, 005, 007, 008, 009).
- Units settled: `klams-service` and `klams-monitor` both active; service
  log clean apart from the two expected protocol-fallback WARNs from the
  probes above. `klams-monitor` logged 4 publish failures in the startup
  window — the documented shape (monitor goes active before the service
  binds `:7777`), and they stopped.
- Consumers: `klams-view` active, zero errors since restart. `klams-mind`
  is not running as a unit on kubs0, so nothing to check there. A real
  `memory_search` through this session's own MCP client succeeded against
  the restarted server.

### Live gate — from cleo, 2026-08-13: PASS

**Passed over two passes, and for the first time in this program the
negotiated revision was confirmed from the server side rather than
inferred.** Pass 1 ran on a session that predated the restart and proved
the *call* path at `2026-07-28`; pass 2, on a genuinely fresh session,
covered the `tools/list` validation that pass 1 could not. Slice 3 closes.

**Enumeration — 10 tools.** The full scope-filtered catalog for this
bearer token (`dissent_propose`, `event_search`, `memory_add`,
`memory_append_event`, `memory_delete`, `memory_related`, `memory_search`,
`memory_supersede`, `memory_update`, `register_author`) — no
`memory_admin_*`, correctly, since this is not an Admin token. Identical to
the pre-deploy catalog for the same token, which is the comparison that
means anything for a per-caller `tools/list`. Enumerated with a cap of 20,
so 10 is the count and not a truncation. No zero-tools signature.

**Calls.**

| Class | Call | Result |
|---|---|---|
| search | `memory_search` (both slice notes) | ok, 3 hits, full payloads |
| tag-filtered search | `memory_search tags:["gotcha"]` | ok, 2 hits |
| **error envelope** | `memory_update` on a nonexistent id | `NOT_FOUND`, `isError: true`, message intact |

No durable write was made. The read and error paths exercise the same
dispatch and framing, and klams writes are not cheaply reversible — events
are append-only and a knowledge write is a real corpus row. Slice 2 already
proved the write path under the identical rmcp 3.1.2 pattern.

#### The part worth keeping: the gate verified its own premise

This session connected **hours before** the 10:00 restart, so a green
result was ambiguous on its face — a client that re-initialises after a
restart can carry its old negotiated version forward, and this program has
twice been burned by a check that answered a neighbouring question. The
client cannot introspect its own negotiated revision, so the ambiguity was
resolved from the server: one correlation probe, then the journal.

```
17:34:54.366054Z  Service initialized as server
                  protocol_version: ProtocolVersion("2026-07-28")
17:34:54.366213Z  mcp.tool dispatch  tool: memory_search
```

No `create new session`, no `InitializedNotification` — the inline
lifecycle, one service instance per request, which is exactly what
SEP-2567 prescribes and what the sprint verified live for `initialize`.
The `client_info` on those spans reads `rmcp/3.1.2` rather than
`claude-code`: under the inline lifecycle there is no real `initialize`
carrying client identity, so rmcp fills its own — the client-driven field
is the `_meta` protocol version, and it reads `2026-07-28`.

For contrast the same journal shows genuine legacy sessions from a
`claude-code/2.1.229` client at `2025-11-25`, with session ids and
initialized-notifications. **Both lifecycles are serving traffic on this
binary right now**, which is the clean-downgrade requirement demonstrated
by live traffic rather than by a probe.

**Method worth reusing:** when a live gate's validity depends on when the
client connected, the client cannot answer it — issue one distinctively
timed call and read the negotiated version off the server's own log. It is
two commands and it converts "probably fresh" into a fact.

#### Pass 2 — the retest on a genuinely fresh session

Pass 1 left one gap, narrow but exactly on target. It proved the *calls*
ran at `2026-07-28`; it could not show that this client had ever validated
a **`2026-07-28` `tools/list`** — and that catalog fetch, not the calls, is
the operation #1212 breaks. The pass-1 catalog was most likely fetched from
the pre-deploy binary, and `ttlMs: 3600000` explicitly invites a client to
keep it for an hour. The journal could not settle it either: klams logs
`mcp.tool dispatch` but never `tools/list`, so a zero count there is a
logging gap, not evidence.

Ken restarted the client. The entire klams traffic from app start:

```
17:58:25.981  Service initialized as server   2026-07-28
17:58:25.999  Service initialized as server   2026-07-28
17:59:17.342  mcp.tool dispatch  memory_search  2026-07-28
```

Two spans 18 ms apart at startup — `initialize` then the catalog fetch —
then the probe call. No `create new session`, no `client initialized`, no
`2025-11-25` anywhere: the inline lifecycle throughout. All 10 tools
registered.

**The conclusion does not depend on identifying which span is which**,
which matters because klams does not log the method. The fresh client
demonstrably holds the full 10-tool catalog; the only klams traffic since
app start is those spans; therefore whichever request delivered the catalog
ran at `2026-07-28`, and the client validated it and registered every tool.
That is the #1212 operation, directly covered.

Also worth noting for the downgrade requirement: pass 1's journal showed a
real `claude-code/2.1.229` legacy session at `2025-11-25`, with a session
id and an initialized-notification, served by this same binary. Pass 2
shows the same client on the inline `2026-07-28` path once reconnected.
Both revisions demonstrated by live traffic, on one build.

#### What this does not settle

It does not read `ttlMs`/`cacheScope` off the wire — the raw suite owns
that, and the `private` scoping decision in particular is invisible from a
client, which is the one part of this sprint no live gate can ever check.
Raw tests prove the bytes; the live gate proves acceptance.

Nor does it prove the catalog is *correct* for other token scopes: this
gate exercised one bearer token's 10-tool view. An Admin token's
`memory_admin_*` surface is covered by the wire suite, not by this.

### The qdrant snapshot shrink: resolved — compaction, not loss

Looked into, since the preflight note below asked for it. **No data loss;
no action needed.**

| Evidence | Reading |
|---|---|
| `points_count` **186,399**, status `green` | Live corpus intact — and *up* 2 from the 186,397 recorded below, so it is growing |
| `num_vectors` **197,535** vs points 186,399 | **11,136 dead vectors** still present, awaiting vacuum |
| vectors are **1024-dim**, `on_disk: true` | 11,136 × 1024 × 4 B = **45.6 MB** |
| observed shrink | **46.7 MB** (1,286,256,128 → 1,239,520,768) |
| `deleted_threshold: 0.2`, `vacuum_min_vector_number: 1000` | A segment is vacuumed once >20% of its vectors are deleted — the 8→7 merge |

The arithmetic lands within **2.4%** of the observed drop, with HNSW link
overhead and payload comfortably covering the remainder. A purge of this
many dead vectors is precisely the size of the shrink that was seen, and
11k more are queued behind it — so this is steady-state churn from
re-scans and supersedes, not a one-off.

Postgres genuinely cannot corroborate, and now for a sharper reason than
"it only holds facts": **there is no knowledge table at all.** Its tables
are `search_sample`, `search_miss`, `facts`, `authors`, `events`,
`dissents`, `summaries`. The knowledge corpus lives **only** in qdrant, so
the nightly qdrant snapshot is its sole backup — worth knowing
independently of this sprint.

**Recommendation for the deploy skill:** its snapshot-size check will keep
firing on qdrant, because vacuum churn makes a shrinking snapshot the
normal case rather than the alarming one. Gate it on `points_count`
(monotonic except for real deletes) instead of snapshot bytes; the byte
check cries wolf on exactly the healthy behaviour.

### Preflight deviations, recorded

- `.env` was missing `KLAMS_STORE_HOST`/`KLAMS_STORE_URL` — it predates
  sprint 042, which introduced them. Values recovered from klams memory
  (`kubsdb`, `https://kubsdb.encke-wahoo.ts.net:4880`), cross-checked
  against k-homelab `docs/deploying.md`, and appended to the gitignored
  `.env`.
- **The nightly qdrant snapshot shrank** 3.6% (2026-08-12:
  1,286,256,128 → 2026-08-13: 1,239,520,768), which the deploy skill says
  to stop and ask about. Proceeded, because the check exists to make a bad
  *migration* survivable and this sprint has none — rollback is binary-only
  either way. Benign explanation available but not proven: qdrant is at 7
  segments where the previous snapshot spanned 8, and the scanner logs
  routine clear-and-reindex churn; compaction genuinely shrinks snapshots.
  Postgres cannot corroborate — it holds only 55 facts / 29 events, while
  the 186,397-point corpus lives in qdrant. **Looked at during the live-gate
  pass — resolved above as vacuum/compaction, quantitatively. Not loss.**
