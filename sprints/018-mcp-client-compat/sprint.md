# Sprint 018 — MCP client compat + auth hot-reload

**Branch:** `018-mcp-client-compat`
**korg:** proposal 308 — WIs #307, #305, #62 (MCP surface), #61 (auth ops)
**Type:** external-client compatibility + operational polish. #307 and
#305 were found live by kyac (2026-07-09) and affect every MCP client
of klams; #62 removes the register-author friction the TokenMaster
spike documented; #61 is the config-reload papercut from the same
spike.

## Goal

Make klams's MCP surface behave correctly for external clients
(Anthropic-bound agents, mcp python-sdk consumers like kyac and
klams-mind), and let bearer tokens be rotated without bouncing the
service.

## Scope

1. **#307 — flatten `memory_add`'s input schema.** `MemoryAddArgs`
   flattens a serde-tagged enum (`MemoryAddContent`, tag `kind`), so
   schemars emits a top-level `oneOf` — which the Anthropic API rejects
   for tool `input_schema`s (400), forcing Anthropic-bound agents to
   drop the tool. Replace the args struct with a flat object schema:
   `kind` as an enum property plus the union of per-kind optional
   fields (`fact_type`, `payload`, `text`, `tags`, `source_path`,
   `repo`); per-kind requirements enforced in the handler with the
   existing `SCHEMA_VALIDATION_FAILED` envelope. The JSON wire format
   is unchanged (the tagged/flattened serde shape already reads
   exactly these fields), so existing callers are unaffected — only
   the *advertised schema* changes shape.

2. **#305 — session-termination DELETE returns 204.** rmcp 1.7's
   `StreamableHttpService::handle_delete` hardcodes 202 Accepted; the
   mcp python-sdk treats only 200/204 as success and logs
   `Session termination failed: 202` on every session close. Add a
   thin response-mapping layer on the `/mcp` mount that rewrites
   DELETE 202 → 204 (empty body). Other methods/statuses untouched.

3. **#62 — auto-register author from bearer identity + relax `repo`.**
   `require_bearer` already stamps `AuthenticatedAuthor` (author_id +
   agent_name from the grant) into request extensions, and rmcp
   forwards `http::request::Parts` into the tool `RequestContext` —
   so `memory_add` / `memory_append_event` / `dissent_propose` can
   fall back to the bearer's bound author when `author_id` is omitted.
   Explicit `author_id` keeps working (register_author flow is
   unchanged and stays backward compatible). The bearer fallback can
   only attribute to the token's own bound author, so no
   cross-identity writes. Also: accept a non-absolute `repo` (bare
   short name) in `register_author`/`memory_add` instead of failing
   with `RepoNotAbsolute`, and say so in the tool descriptions.

4. **#61 — hot-reload `[[auth.tokens]]`.** SIGHUP handler in
   klams-service re-reads `klams.toml`, re-resolves token→author
   bindings, and atomically swaps the grant table shared by REST and
   MCP (`AuthState` moves to a swappable grant list). Added tokens
   authenticate and removed tokens stop authenticating after reload;
   in-flight requests are unaffected (they hold the old snapshot for
   the duration of their auth check only). `/etc/klams` permission
   model unchanged. Documented in `docs/setup.md`.

## Out of scope

- #63 (Grafana search/context latency panel) — stays in korg proposal
  178 ("klams small-tools polish").
- Changing token storage format or moving tokens out of the TOML.
- File-watch-based config reload (explicit signal only).
- Reloading non-auth config sections on SIGHUP (tokens only; the rest
  of the config keeps requiring a restart).

## Acceptance

- `memory_add`'s advertised `input_schema` has no top-level
  `oneOf`/`allOf`/`anyOf`; a test asserts this structurally for
  **all** advertised tools. Per-kind validation errors surface as
  `SCHEMA_VALIDATION_FAILED`, and valid fact/knowledge payloads in the
  existing wire shape still round-trip.
- HTTP DELETE on `/mcp` with a live session id returns 204 No Content.
- An authenticated bearer caller with a bound agent_name can
  `memory_add` without prior `register_author`; the write is
  attributed to the bearer's author. A caller with no bound author
  (legacy token) attributes to `system` as today.
- `register_author` accepts `repo: "krag"` (short name) as well as
  absolute paths.
- SIGHUP reload: new token authenticates, removed token 401s, no
  restart. Covered by a test at the `AuthState` swap level; live
  SIGHUP verified on kubs0 at deploy.
- Docs: `docs/usage.md` (MCP tool notes) + `docs/setup.md` (reload
  recipe) updated; `just gate` green.

## Chronicle

- (2026-07-09) Sprint opened from korg proposal 308. Scope = the two
  kyac-found MCP compat issues (#307, #305) + pulled-in #62 (same MCP
  write path) and #61 (Ken's pick). #63 deliberately left behind.
- Recon notes: rmcp 1.7 `handle_delete` returns `accepted_response()`
  (202) — not configurable, hence the mount-level response map.
  `McpState.grants` is currently unused by tool modules (was plumbed
  in 007 for exactly the #62 use case but never consumed) — the
  bearer-author fallback reads request extensions instead, which is
  where the truth already lives.
- (2026-07-09) All four items implemented, TDD throughout:
  - **#305** `delete_status_compat` layer in `klams_mcp::router()`
    (202→204 on DELETE only). Hermetic tests in
    `klams-mcp/tests/delete_status_compat.rs`; live session test in
    `klams-service/tests/mcp_session_delete.rs`. Surprise: axum's
    `Route` re-adds `content-length: 0` after router-level middleware,
    so the in-process test can't assert header absence — hyper strips
    it at the wire and the python-sdk only checks status, so the test
    pins status + empty body only.
  - **#307** `MemoryAddArgs` flattened (`kind` enum + per-kind
    optional fields); per-kind requirements enforced in
    `MemoryAddArgs::content()` with `SCHEMA_VALIDATION_FAILED`. Wire
    shape unchanged — pinned by `tests/memory_add_args.rs`. New
    contract test `tests/tool_schemas.rs` walks
    `all_tool_descriptors()` (extracted from `list_tools`) asserting
    no advertised tool has top-level `oneOf`/`allOf`/`anyOf`/`$ref`.
  - **#62** central bearer-author fallback in `call_tool` (injects
    `author_id` into the args JSON when omitted, for
    `memory_add`/`memory_append_event`/`dissent_propose`); explicit
    `author_id` always wins. Repo validation relaxed to non-blank
    (`RepoNotAbsolute` → `RepoEmpty`). `register_author` is now
    optional for bearer-bound callers — server instructions +
    descriptions updated. Live coverage:
    `klams-service/tests/mcp_bearer_author.rs` (TestServer gained an
    author-bound token).
  - **#61** `AuthState` grant table moved behind an `RwLock`
    (`replace_grants` swaps atomically, all clones share it);
    SIGHUP task in main re-reads config, validates `[auth]`, rebuilds
    grants (re-resolving author bindings), swaps — failed reloads keep
    the old table. `ExecReload=kill -HUP` added to the systemd unit.
    Swap behavior pinned by a live-router test in
    `klams-api/src/auth.rs`; live SIGHUP to be verified on kubs0 at
    deploy.
  - Rode along: removed the dead `McpState.grants` field (would have
    gone stale under #61); fixed stale `mcp_auth.rs` scope-surface
    expectations that sprint 015 missed (dissent_propose — those tests
    are `#[ignore]`d, outside the CI gate, so the drift hid); usage.md
    tool table corrected (`memory_context` was never mounted as an MCP
    tool; `dissent_propose` was missing).
  - Known-failing before and after (environmental, untouched):
    `perf_smoke::search_p95_under_500ms_at_mvp_corpus` fails
    identically on `main`.
  - Full `--ignored` integration suite (compose stack): 54 passed /
    1 failed (the pre-existing perf_smoke above).
- (2026-07-09, late adds at Ken's request)
  - **Version convention**: workspace version PATCH segment now
    tracks the sprint number (`0.1.18` for this sprint) so the
    version on `/healthz` / MCP `server_info` — which Ken's dashboard
    shows — identifies the deployed sprint at a glance. Recorded as a
    standing rule in AGENTS.md step 2 (set it when opening the sprint
    doc); starts at 018, MAJOR/MINOR stay hand-managed.
  - **Agent handoff doc**: `docs/klams-mcp-for-agents.md` — a
    self-contained page Ken can point any agent (Claude Code,
    Copilot, other MCP clients) at to enable the klams MCP (global
    and per-repo variants) plus a copy-paste instruction blurb that
    positions klams as the cross-agent memory store. Linked from
    README. Prompted by the discovery that Claude Code was never
    actually wired to the klams MCP.
- (2026-07-09) **Deployed to kubs0** from this branch pre-merge:
  release build → `deploy/install-systemd.sh` (picks up the new
  `ExecReload=` unit) → restart. `/healthz` reports `0.1.18`. Live
  SIGHUP acceptance verified: `systemctl reload klams-service` →
  journal logs `SIGHUP: auth token table reloaded` (grants=13), no
  restart, service stayed healthy.
