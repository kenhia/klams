# Sprint 039 — Retire the viewport; klams-view is the human surface

**Proposal:** korg:742 (covers #802 Remove the parked viewport app;
point docs at `kenhia/klams-view`)
**Started:** 2026-07-30 · **Version:** 0.1.39
**Type:** removal + docs. No behaviour change to the service.

## Goal

Delete the Tauri `viewport/` app from this repo and hand the human
surface off, in the docs and in every user-visible string, to
[kenhia/klams-view](https://github.com/kenhia/klams-view).

## Why now

Proposal 742 started life as *"fix the red goo, then make the viewport
worth opening"* (#739 + #740). It was parked at the bottom of the queue
on 2026-07-29 for an explicit reason: Ken wanted to run a greenfield
viewport experiment in a sibling project and let the result decide
#740's rewrite-vs-refactor question by evidence rather than argument.
The parking comment committed in advance to the consequence — *"if the
greenfield wins, this proposal gets re-scoped or declined rather than
done."*

The greenfield won. **klams-view** is a published, deployed project of
its own: one axum binary serving a SvelteKit SPA, an `/api/*`
aggregation layer that calls klams server-side, a systemd unit on the
klams host, its own sprint records and roadmap. It beats the refactor
path on the things that actually mattered:

| | viewport (Tauri) | klams-view |
|---|---|---|
| Token | in the client's OS keyring, reachable by the webview | server-side only; the browser never sees it, and it is **read**-scoped |
| Reach | a cross-compiled `.exe` hand-shipped to one Windows box | a URL any browser on the tailnet can open |
| Aggregation | whatever REST returns, rendered raw | activity/metrics history, corpus share — computed server-side, things the klams REST API does not expose |
| Upkeep | a second Cargo workspace + a pnpm tree + its own CI job in *this* repo | its own repo, its own gate |

So #739 and #740 are resolved, not carried: #739's `[object Object]`
bug dies with the code (and cannot recur in klams-view, whose client
throws `ApiError extends Error`), and #740's item 4 *was* the decision.
The live residuals moved to the project that now owns the surface —
klams-view #807 (author-view residuals), #808 (connection doctor),
#809 (live-backend smoke test).

## Scope

1. **Delete `viewport/`.** Its own Cargo workspace, so nothing in the
   service workspace depends on it. `klams-client` stays —
   klams-service, klams-monitor, klams-scanner and `tools/bench` all
   use it.
2. **Strip the scaffolding**: the `viewport-*` and `gate-viewport` /
   `gate-all` justfile recipes and the `VIEWPORT_HOST` /
   `VIEWPORT_DEPLOY_DIR` vars; the `viewport` CI job; the viewport
   entries in `.gitignore` / `.dockerignore`; the viewport checks in
   `scripts/verify-mvp.sh`.
3. **Re-point the docs** — README, architecture, setup, usage, install,
   auth, `klams-mcp-for-agents`, the example config, the topology
   diagram, the pitch pages, and the `AGENTS.md` directory table.
4. **Fix user-visible strings in the crates**, which is where a stale
   viewport reference does real damage: `dissent_propose`'s MCP tool
   description told every agent that "a human resolves it in the
   viewport."

Historical sprint records (`sprints/001`–`038`, 77 files mentioning the
viewport) are chronicles and stay untouched.

## Decisions taken during the sprint

### D-1 — Dissent resolution is documented as REST, not as a UI

The viewport was not only a display: it was the **curation surface**.
It held `["read", "write", "manage"]` precisely because a human
resolved dissents there, and `docs/auth.md` had a standing note
explaining why a read-only viewport token was wrong.

klams-view does not replace that. It is deliberately read-only, and
dissent promote/discard sits in its roadmap under *Later*. Renaming
"viewport" to "klams-view" in the auth docs would therefore have
shipped a lie in both directions — it would claim klams-view can
curate, and it would keep recommending a `manage` token for a
component that wants `read`.

So the docs now describe the real resolution path, which existed all
along underneath the app:

```
POST /v1/memory/dissents/{id}/promote    # manage scope
POST /v1/memory/dissents/{id}/discard    # manage scope
```

The `manage` scope keeps its rationale, but it is attached to the
operator/curation *credential* rather than to a GUI, and the auth docs
say plainly that there is currently no UI for it. `dissent_propose`'s
tool description now tells agents a human resolves the proposal over
the dissents endpoints — true today, and it stops promising a screen
that no longer exists.

### D-2 — Read-only by default is now the recommended UI posture

The pre-025 advice ("give the UI a read-only token so a UI compromise
cannot mutate state") was refuted for the viewport, because the
viewport's own features needed `write` + `manage`. With the curation
UI gone, the advice is correct again for the component that replaced
it — klams-view genuinely needs nothing but `read`. `docs/auth.md`
records the reversal along with why it is not a flip-flop: the posture
follows what the client actually does, and the client changed.

### D-3 — The topology diagram gains an HTTP hop

`docs/diagrams/klams-topology.svg` drew the viewport as a desktop app
holding a bearer token and calling klams directly. klams-view is a
*server* that holds the token and serves a browser, so the diagram
needed a redraw rather than a relabel: browser → klams-view (:7778) →
klams (:7777), with the token annotated on the server-side hop.

## Acceptance criteria

- `viewport/` is gone; `just gate` and the integration suite pass.
- No `viewport` reference survives outside `sprints/` (historical
  records) and this sprint doc, except where it is deliberately
  describing history.
- Every doc that named the viewport as the UI now names klams-view and
  links to `https://github.com/kenhia/klams-view`.
- No user-visible string (MCP tool description, error text, example
  config) mentions the viewport.
- The dissent-resolution path is documented truthfully as REST.

## Outcome

Done as scoped. 62 tracked files deleted, 27 modified.

**Removed:** `viewport/`; the `viewport-build` / `viewport-deploy` /
`viewport-build-linux` / `viewport-run-linux` / `gate-viewport` /
`gate-all` recipes and the `VIEWPORT_HOST` / `VIEWPORT_DEPLOY_DIR`
vars; the CI `viewport (svelte + tauri checks)` job; the viewport
entries in `.gitignore` and `.dockerignore`; `verify-mvp.sh`'s SC-006
(now `n/a`) and SC-007's dependence on `viewport/README.md` (now
`docs/install.md`).

**Docs re-pointed:** README, `docs/{architecture,setup,usage,install,auth,klams-mcp-for-agents}.md`,
`deploy/config/klams.example.toml`, `docs/pitch/klams-pitch.html`,
`AGENTS.md`'s directory table, and `docs/diagrams/klams-topology.svg`
(redrawn per D-3, subtitle now 0.1.39).

**Strings fixed in the crates:** the `dissent_propose` tool description
(the one that told every agent to use a screen that no longer exists),
plus module docs and comments in `klams-core` (`policy.rs` — the
`Source::User` description that shipped over the API, `projection.rs`),
`klams-types`, `klams-store`, `klams-client`, `klams-api` and
`rest_route_scopes.rs`.

**Sections rewritten rather than renamed**, because klams-view does not
do what the viewport did: `usage.md`'s "Viewport: provenance panel +
Dissents page" (now a historical note pointing at the REST recipe),
"Context Preview pane" (now describes `POST /memory/context` itself),
the `/authors` review workflow and the Activity tab (now described as
the REST endpoints they always were, with klams-view named as the
comfortable way to read them). `auth.md` gained
[Resolving dissents without a UI](../../docs/auth.md) with the actual
curl recipe.

**Verification:** `just gate` green. `just test-integration` green on
the second run — **125 passed, 0 failed**.

### One flake, not ours

The first integration run failed one test:

```
task_fact_decays_faster_than_user_fact
tick_once: BackendUnavailable("apply_decay_batch: error returned
from database: deadlock detected")
```

A Postgres deadlock inside `apply_decay_batch` when decay tests run
concurrently with the rest of the suite at default parallelism. It
passed on re-run and on a targeted re-run, and nothing in this sprint
touches decay, the store's write path, or transaction ordering — it is
a pre-existing intermittent, filed as **#811** rather than swept under
"re-run it". Worth noting because sprint 031 (#679) removed
`--test-threads=1` from this suite on the grounds that the shared-table
race was fixed; this looks like a second, narrower one that survived.

### Disk

Deleting the tracked source left ~21 GB of untracked build artifacts
behind (`target/` 21 G, `node_modules/` 66 M, plus `build/`,
`.svelte-kit/`, `src-tauri/gen/`). Verified to contain zero
non-artifact files, then removed on Ken's go-ahead. The source remains
recoverable from git history.

## Deployed 2026-07-30

- Version `0.1.39` live on kubs0 (`/healthz` and MCP `server_info` both
  confirm; was `0.1.37` — 038 was a docs-only sprint and never deployed).
- Rollback target: `0.1.37` via `just rollback` (`.prev` binaries in
  place for all three).
- Migrations applied: **none** — this sprint touched no `migrations/`,
  so the rollback is a clean binary swap with no restore needed.
- Verified live, beyond `/healthz`: `just verify` (7 passed, 0 failed —
  including the two SC checks this sprint rewrote: SC-006 now reports
  `n/a: UI retired`, SC-007 checks `docs/install.md`); MCP `initialize`
  returns `0.1.39` and `tools/list` advertises 16 tools; and the one
  string this sprint actually changed in the running service —
  `dissent_propose`'s description — now reads "…a human promotes or
  discards over the dissents endpoints" instead of naming the viewport.
  Units settled with `NRestarts=0` and no ERROR/WARN in the log.

### Config follow-up for the operator (not done by this sprint)

`/etc/klams/klams.toml` is outside this repo and was deliberately not
touched. It still carries the now-dead grant:

```toml
label = "viewport"
agent_name = "viewport"
scopes = ["read", "write", "manage"]
```

Nothing uses it — the app it belonged to no longer exists, and
klams-view has its own read-scoped `klams-view` grant (klams-view #793,
already present in the deployed config). It is a live `manage`-scoped
credential with no owner, which is the kind of thing worth removing on
principle rather than leaving to rot. Deleting the block and
`sudo systemctl reload klams-service` (hot-reloads `[[auth.tokens]]`,
no restart) is all it takes.
