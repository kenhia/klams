# Phase 0 — Research: Non-Agentic Writes, Integrations, and the Systemd Switchover

**Sprint**: 003-non-agentic-writes
**Date**: 2026-05-18
**Reading order**: this file documents the technical decisions that
underpin [plan.md](plan.md) and the contracts/data-model. Each entry
follows the same shape: Decision → Rationale → Alternatives considered.

## 1. Scanner: separate binary vs in-process tokio task

**Decision**: separate binary in its own workspace crate
(`klams-scanner`), invoked by a systemd timer.

**Rationale**:

- Scans are bursty and CPU-bound (chunking + hashing); embedding them
  inside the always-on `klams-service` would couple the scanner's
  resource profile to the API's tail-latency budget.
- A failing scan must not crash the API service. A crashed scanner is
  invisible to API consumers and trivially restarted by systemd.
- The scanner only needs the public HTTP surface
  (`POST /memory/knowledge/index`) — it has no privileged access. The
  same isolation Phase 3 wants for Ansible callbacks applies here.
- Keeps the scanner reusable as an ad-hoc CLI for one-shot reindexing
  (`klams-scanner --once --path ~/src/foo`).

**Alternatives considered**:

- In-process tokio task inside `klams-service`: rejected — couples
  process lifecycle and resource limits; harder to operate
  independently; harder to test in isolation.
- Cron + python script: rejected — adds a second toolchain to deploy;
  no shared types with the rest of the workspace.

## 2. Cursor store: sqlite vs Postgres table vs flat JSON

**Decision**: local sqlite file at
`${XDG_STATE_HOME:-$HOME/.local/state}/klams/scanner.sqlite` with a
single table.

**Rationale**:

- Cursor data is per-host operational state, not project memory. It
  doesn't belong in the `klams` Postgres database (which is the shared
  memory store) nor in Qdrant (which holds embeddings).
- sqlite gives us atomic upserts and a real transaction boundary for
  free, which a flat JSON file cannot offer without bespoke
  fsync-and-rename logic.
- Scoped to the scanner's host. The systemd unit's `StateDirectory=`
  directive makes the path stable and clean-on-uninstall.

**Alternatives considered**:

- New Postgres table `scanner_cursors`: rejected — adds a schema-shape
  change to a tightly-versioned production DB for purely operational
  per-host state. Increases blast radius of a botched migration.
- Flat `.json` file rewritten on every commit: rejected — lossy on
  concurrent writes; would need a custom mutex; no atomicity story
  for partial scans.

## 3. Walk implementation: `walkdir` vs `ignore` vs hand-rolled

**Decision**: the [`ignore`](https://crates.io/crates/ignore) crate
(the one ripgrep uses).

**Rationale**:

- Honors `.gitignore` semantics out of the box, which is exactly the
  syntax FR-006 mandates for `.klamsignore`. Re-implementing
  gitignore matching is a known footgun.
- Already does the standard skip set (`.git/`, etc.) plus parallel
  walking; matches our perf target for cold scans.
- Pure Rust, no system deps; portable across kubs0 and dev boxes.

**Alternatives considered**:

- `walkdir` + hand-rolled glob matching: rejected — would re-implement
  gitignore precedence, which is subtler than it looks (negations,
  trailing slashes, anchored patterns).
- `find(1)` shell-out: rejected — slow IPC, no structured filter
  composition, brittle parsing.

## 4. Chunking strategy

**Decision**: paragraph-bounded chunks of roughly 800 characters with
a 200-character overlap; markdown heading boundaries are hard breaks
(never split across `## ` or higher). Each chunk's sha256 over its
post-normalized text is the dedupe key.

**Rationale**:

- Matches the size band the Phase 1 embedding model
  (`bge-small-en-v1.5` per the Phase 1 research notes) was trained
  on; goes through the existing `POST /memory/knowledge/index`
  endpoint unchanged.
- Markdown heading awareness keeps `~/obsidian/` notes coherent —
  each chunk is roughly a single thought.
- Character-based bounds avoid bringing a tokenizer into the scanner;
  the embedding side already runs the tokenizer on the server.

**Alternatives considered**:

- Fixed line counts: rejected — code files with very long lines
  produce overflowing chunks; prose files produce too many.
- Token-bounded with on-host tokenizer: rejected — adds a heavy
  dependency for a small accuracy gain at this phase.

## 5. Service monitor: poll `systemctl` vs subscribe to dbus

**Decision**: poll `systemctl is-active <unit>` every 15s for the
configured unit list, diff against an in-memory previous state,
emit `service.*` events on transitions.

**Rationale**:

- Trivially testable: swap `systemctl` for a fake binary on `PATH`
  that prints scripted output. We do exactly that in `us3c_monitor.rs`.
- No dbus dependency on the host; no privileged socket. The monitor
  runs as the `klams` system user.
- The 15s poll interval keeps worst-case detection latency comfortably
  inside SC-003's 30s end-to-end budget (a 30s interval put the
  worst case right at the boundary).

**Alternatives considered**:

- dbus subscription to systemd's `UnitFiles.PropertiesChanged`:
  rejected for this sprint — a real dependency and a real perms story.
  We can swap to dbus later if the poll budget ever becomes a problem;
  the in-memory state-diff layer doesn't care which producer fed it.
- `journalctl --follow -u <unit>`: rejected — text parsing of
  human-readable output; brittle across systemd versions.

## 6. `path` field placement on write responses

**Decision**: add `path` (always present) and `dissent_id` (present
only when `path == "dissent"`) as **additive** fields on the existing
write-endpoint response bodies. No existing fields move or change
type.

**Rationale**:

- Phase 1 and Phase 2 clients (the CLI, the viewport, the controller
  client) all use `serde_json::from_value` with `#[serde(default)]`
  on unknown-field-tolerant structs; an additive field is back-compat.
- The Phase 2 dissent response already returned `dissent_id` when a
  write diverted, but did so implicitly via a separate response shape.
  Folding into one shape keeps integrators from having to switch on
  status codes to decide what fields to read.

**Alternatives considered**:

- A wholly new envelope (`{ "path": ..., "result": {...} }`): rejected
  — breaks Phase 1/2 clients; large blast radius for a small
  observability gain.
- A response header (`X-KLAMS-Write-Path`): rejected — invisible to
  most existing client codepaths; harder to log; not a natural fit
  for the typed Rust `klams-client` crate.

## 7. `GET /memory/policy` shape

**Decision**: return a single JSON object keyed by source name,
each value carrying `rank` (integer, higher = more trusted) and
`description` (string, free-form). Derived from the same Rust enum
the dispatcher uses via a `From<&PolicyTable>` impl; a unit test in
`klams-core` asserts the JSON projection equals what the dispatcher
holds.

**Rationale**:

- Single round-trip; no need for paging at four entries.
- Keying by source name (rather than an array of `{source, rank}`
  objects) is what integrators want to look up — they ask "what rank
  is `Task`?" not "give me everything sorted by rank."
- The `From<&PolicyTable>` impl is the single source of truth; the
  contract test prevents the JSON from drifting from the dispatcher.

**Alternatives considered**:

- Static JSON file served from disk: rejected — would drift from the
  in-memory dispatcher; defeats the whole point of FR-018.
- Per-source endpoints (`GET /memory/policy/{source}`): rejected —
  premature; small table fits in one response.

## 8. Binary rotation strategy

**Decision**: install recipe writes the new binary to a temp path next
to the live path, then renames the live binary to
`klams-service.prev` and the new one to `klams-service`, both with
`rename(2)` (atomic on the same filesystem). One follow-up call to
`systemctl restart klams-service`.

**Rationale**:

- `rename(2)` is atomic — there is no window where the path is
  missing or partially written, even if power is yanked mid-flight.
- Keeps both binaries available so a botched start can be reverted
  with `mv klams-service.prev klams-service && systemctl restart`.
- Uses standard POSIX semantics; no platform-specific magic.

**Alternatives considered**:

- `cp` followed by `chmod +x`: rejected — non-atomic; observable
  partial state.
- Symlinks (`klams-service` -> `klams-service-2026-05-18.abc123`):
  rejected — adds a layer of indirection without a clear win at
  current scale; the rotation pattern is what most systemd unit
  authors expect to see.

## 9. Handoff document layout

**Decision**: stage the handoff under
`specs/003-non-agentic-writes/handoff/` during the sprint with this
shape:

```text
handoff/
|-- README.md         # one-page orientation, pinned to klams 003
|-- spec.md           # speckit-compatible spec for the ansible-k side
|-- api-contract.md   # endpoint table, payloads, failure modes
`-- examples/
    `-- post-userfact.sh   # minimal curl walkthrough
```

At sprint ship, `cp -r specs/003-non-agentic-writes/handoff/
/home/ken/ansible-k/specs/klams-integration/` is one of the final
tasks. The ansible-k owner then runs their own speckit `/clarify` →
`/plan` → `/tasks` cycle against the staged spec.

**Rationale**:

- Authoring the handoff inside this repo means it goes through the
  same `just gate` (markdown links, no broken refs) the rest of our
  docs do.
- Mirroring speckit's expected layout (`spec.md` etc.) means the
  ansible-k owner can run a normal speckit cycle without first
  reshaping the docs.
- A pinned-version header in `README.md` plus a "how to detect drift"
  section per FR-021 gives the receiving project a concrete signal
  when klams' API surface changes.

**Alternatives considered**:

- Author directly in `/home/ken/ansible-k/`: rejected — the writing
  loop wants `just gate`, link-checking, and the spec-driven cycle
  that already lives here.
- A single long markdown file instead of a directory: rejected —
  doesn't match speckit's structure; harder to evolve incrementally.

## Open questions resolved

The spec contained no `[NEEDS CLARIFICATION]` markers. The following
decisions that the spec deliberately deferred have been cemented
above:

| Spec hook | Decision (above) |
|-----------|------------------|
| FR-004 "scanner binary or in-process tokio task" | §1 — separate binary. |
| FR-007 metrics names | §1 — emitted by `klams-scanner` per FR-007's prefix; no spec changes. |
| FR-011 "klams-monitor systemd unit or equivalent" | §5 — separate binary + dedicated systemd service unit. |
| FR-016 "path field" placement | §6 — additive on existing write responses. |
| FR-018 "policy endpoint" shape | §7 — keyed JSON object. |
| Handoff location | §9 — staged in-repo, copied at ship. |

No items remain for `/speckit.clarify` to resolve.
