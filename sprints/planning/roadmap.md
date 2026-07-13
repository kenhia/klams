# klams — roadmap

**Status:** Active — this is the pointer document: the top entry under
"Sprint queue" is the next sprint. Sprints may also arrive from korg
sprint proposals; when they do, move/merge queue entries accordingly.  
**Date:** 2026-07-10 (fresh start after the crossroads review; sprints
001–020 shipped; the 021–024 queue and §5 findings are now filed as korg
klams work items, bundled into sprint proposals — 021 is in flight)  
**Related:** [2026-07-crossroads.md](2026-07-crossroads.md) (the review
this queue derives from) · [archive/](archive/) (pre-020 planning,
historical) · companion projects: `~/src/ai/klams-mind` (memory
intelligence), korg `homelab-ai` project (cross-repo workstreams)

## Where we are

Plumbing era complete (001–020): memory model, MCP/REST surfaces,
ingestion, backups, telemetry — all live on kubs0 and verified in use
by Claude Code, GHCP, kyac, and klams-mind. The next era is **retrieval
quality and memory value**: agents are the customers now, and the
observed weaknesses are chunk quality, exact-identifier recall, and no
feedback loop from agent use back into tuning.

Standing decisions:

- **klams stays Rust; intelligence lives in klams-mind** (WI-259,
  2026-07-05, archived).
- **No store migration without eval evidence** (crossroads §2.1; kris
  precedent). OpenSearch may join as an additional lexical source if
  the evals demand it — trial, not swap.
- **Version convention:** workspace PATCH = sprint number (AGENTS.md).

## In flight

### 021 — Corpus hygiene + miss log → [sprints/021-corpus-hygiene/](../021-corpus-hygiene/sprint.md)

Shipped 2026-07-10 (PR #23, `903fffd`; deployed 0.1.21 on kubs0).
delete-before-reindex, scanner file-type allowlist, miss log
(`search_miss` + `klams_search_misses_total` + Grafana panel),
routing-rules agent blurb propagated to kubs0/kai/cleo. korg WIs
#315–#318 resolved, proposal `korg:337` done. Detail in the sprint doc.

### 022 — Scanner v2: chunks worth retrieving → [sprints/022-scanner-v2/](../022-scanner-v2/sprint.md)

Shipped 2026-07-11 (PR #24, `ee8aa7b`; deployed 0.1.22 on kubs0, full
corpus re-indexed 48k→73k with heading-path chunks + tree-sitter symbol
extraction). Language-aware chunker, chunk metadata to payload, per-file
dedupe, embed_batch, golden tests. Also repaired main CI (red since
≥018). korg WIs #320–#327 resolved, proposal `korg:338` done.

### 023 — Multi-host scanning + host identity → [sprints/023-multi-host-scanning/](../023-multi-host-scanning/sprint.md)

Shipped 2026-07-13 (PR #25, `5762a8f`; deployed 0.1.23 on kubs0 + a
per-host scanner provisioned on kai). Host stamped on every chunk;
delete + dedupe host-aware `(machine, file)`; host in the knowledge
projection. kai's `/home/ken/src` now in the corpus (`host=kai`); kubs0
backfilled to `host=kubs0` in place via Qdrant `set_payload` (no
re-index). korg WIs #407–#411 resolved, proposal `korg:413` done. Remote
scanners reach klams via the tailscale-serve HTTPS MagicDNS URL. NFS
central mount-scan captured as klams #406.

### 024 — One ranking: fusion unification + eval enablement → [sprints/024-ranking-unification/](../024-ranking-unification/sprint.md)

Started 2026-07-13 from korg proposal `korg:339` (covers klams WIs
#328–#332). Make MCP `memory_search` (the real-traffic
surface) use rank-based fusion instead of raw cross-scale score sort;
converge the three merge implementations on `hybrid::fuse`; route
klams-mcp through the Store/adapter seam instead of concrete
`CompositeStore` internals; wire (or delete) the dead `[retrieval]
fusion` config; hermetic merge-invariant tests so ranking can't regress
off-main. This is the 016-deferred work, now due — and the structural
prerequisite for any third search source. Klams-side surface for
klams-mind's identifier-heavy eval suite rides along.

## Sprint queue

### 025 — Decide & do: lexical knowledge search (gated on data)

If the miss log + evals confirm the exact-identifier gap: add a lexical
source for knowledge behind the now-unified fusion — candidates, cheap
first: (a) Qdrant full-text payload index (match-based, no BM25
ranking), (b) Postgres FTS mirror of chunk text, (c) BM25 from the
already-running OpenSearch instance (which also buys score
normalization, per-field analyzers, filtered kNN — the kris trade
study). If the data says the gap isn't real: decommission the idle
OpenSearch container and close the question. Either way this sprint
ends the "Qdrant or OpenSearch" ambiguity with evidence.

### 026 — Graph memory spike (timeboxed)

The TokenMaster F1 on-ramp (crossroads §2.2 #2): symbol/edge schema,
scanner-emitted edges (embed/shell graphify or port its heuristics —
scanner v2's symbol extraction is the first half), `callers`/
`callees`/`impact` MCP verbs with token-bounded caps. Outcome is a
go/no-go on the graph as a first-class klams layer, not a commitment.

### 027 — Capability index feeder

Crossroads §2.2: ingest structured sources — korg (WIs, reports,
proposals), kvllm eval results, deployed-service inventory — as
knowledge with source/kind metadata, so "what can this homelab do /
where is X tracked" is one `memory_search` away. klams stays the
index, never the system of record; staleness = re-scan. Can be pulled
earlier if agent demand shows up in the miss log.

### Breather (slot between any two of the above) — upgrades

Crossroads §5 #10/#11: axum 0.8 (+ axum-prometheus lockstep), thiserror
2, metrics 0.24, Qdrant `query_points` (also the door to server-side
hybrid), Prometheus/Grafana image refresh, pin `rust-toolchain.toml`,
delete the dead code. All mechanical now, blocking later.

### Later / unscheduled

- **Usefulness signal** (`memory_feedback` + `useful_count` boost in
  decay) — enablement for klams-mind consolidation (korg #271/#272);
  pull forward when klams-mind gets there.
- **Multi-vector embeddings + embedding-model upgrade** — after 022;
  the 014 re-embed runbook + 022's batch path make it tractable.
- **Knowledge decay** — decide once usefulness data exists.
- **Backup-stale alerting** (Grafana alert rules from the ansible-k
  handoff — 020 made the metrics real).
- Viewport: source/trust + decay-weight surfacing; self-update; code
  signing.
- Cross-machine caching; multi-agent scratchpad; cloud backup sync.

## How to start the next sprint

Per [AGENTS.md](../../AGENTS.md): take the top queue entry, create
branch + `sprints/###-<short-stub>/` (next number), set the workspace
version PATCH to the sprint number, write `sprint.md` (goal, scope,
acceptance — the queue entry above is the seed), build test-first, keep
the chronicle current, ship behind `just gate`. Move the entry out of
this queue when its sprint doc exists.
