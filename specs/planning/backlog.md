# klams backlog

Simple backlog for this project. User and agent will add items for future
consideration.

See [plan.md](plan.md) for the phased roadmap and [viewport.md](viewport.md)
for the desktop GUI plan. Items below are deferred or not yet scheduled.

## Agent Instructions

1. New items for the backlog should be created with a section header `## Some feature`
2. Items that have been added to sprint or cut should be removed from this file
   and placed in `specs/planning/backlog-archive.md`
    - If added to a spec/sprint, a markdown-link to the new feature spec should
      be added immediately following the seciton header
    - If the item is moved due to being cut ` **CUT**` should be added to the
      title (i.e. "## Some feature" becomes "## Some feature **CUT**")

## Multi-vector embeddings (text + code)

Separate embedding spaces for prose vs source code, with per-space retrieval
weighting. From Phase 7 of the original plan.

## Lightweight graph memory

Add a relationship/edge layer over facts and knowledge items for multi-hop
queries.

## Memory diffing and replay

Snapshot memory state and compute diffs over time; replay agent sessions
against historical memory.

## Cross-machine caching

Local cache layer on controller machines to reduce round-trips to `kubs0`.

## Multi-agent coordination memory

Shared scratchpad for agents collaborating on the same task.

## Viewport self-update

Tauri updater integration so the viewport pulls new builds without manual
installer copies.

## Viewport code signing

Sign the Windows installer to silence SmartScreen warnings if they become
disruptive.

## Cloud backup sync for klams

Optional sync of `gratch` backup artifacts to off-site cloud storage. Depends
on the existing gratch backup chain.

