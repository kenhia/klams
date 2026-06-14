# klams — current path forward

**Status:** Active planning — basis for the next sprint  
**Date:** 2026-06-09  
**Owner:** Ken (with GHCP)  
**Related:** [plan.md](plan.md) · [backlog.md](backlog.md) · [tokenmaster-integration/](tokenmaster-integration/README.md)

This document is the agreed snapshot of where klams is, what the next sprint
will be, and the decision on migrating the planning toolchain from Spec Kit to
ATV-StarterKit. It is the working reference until the next sprint spec exists.

---

## 1. Where we are (refresher)

### Shipped

Sprints 001 → 009 are all merged to `main` (latest: `f1ced5a feat(009):
Stability & Attribution`). That includes:

- **001–002** — MVP memory (facts/events/knowledge, search, decay) + safety,
  dissents, viewport inspector.
- **003** — non-agentic writes: `klams-scanner` and `klams-monitor` binaries
  built, unit files + `install-systemd.sh` written.
- **004–006** — EnvFact JSON values, advanced retrieval/summarization,
  maintenance + backups.
- **007** — MCP memory server (`klams-mcp`): `memory_search`, `memory_related`,
  `memory_add`, `memory_append_event`, `event_search`, `register_author`.
- **008** — activity & observability + perf baseline.
- **009** — stability & attribution: closed the CLOSE_WAIT fd-leak (kwi #26) and
  the viewport author→memory 404 (kwi #28), one-shot re-attribution migration,
  refreshed perf baseline.

### Running on `kubs0` (verified 2026-06-09)

| Unit | State | Note |
|------|-------|------|
| `klams-service.service` | ✅ installed, enabled, active | the API/MCP/queue binary |
| `klams-scanner.timer` | ❌ **not installed** | nothing is indexing `~/src` / `~/obsidian` |
| `klams-monitor.service` | ❌ **not installed** | a legacy `~/src/tools/ksvc-looper/klams_monitor.py` runs instead |

**Key gap:** the scanner and monitor were *built and CI-green in sprint 003*,
but the systemd switchover only landed for the service. This is **deployment
debt**, not missing code. Its consequence is material: **knowledge memory is
likely empty/stale** (no scan cycle is running), so klams's semantic search is
operating on whatever was hand-indexed, and service events come from the old
python looper rather than the typed Rust monitor.

### Open work items (kwi `klams`)

| # | Type | Area | Title |
|---|------|------|-------|
| 31 | bug | viewport | Memory detail routes (`/facts/[id]`, `/events/[id]`, `/knowledge/[id]`) return 404 |
| 32 | bug | service | Authors `counts.writes` excludes `knowledge_items` |
| 33 | bug | service | `just bench-clean`: Qdrant delete should use `?wait=true` to drain synchronously |

### Backlog themes worth tracking

From [backlog.md](backlog.md): **Lightweight graph memory** (the TokenMaster
overlap — a relationship/edge layer for multi-hop queries), **multi-vector
embeddings** (text + code), **usefulness-signal decay boost**, and several
viewport surfacing asks (source/trust rank, decay weights).

---

## 2. Next sprint — "Operationalize ingestion" (Option A) + TokenMaster spike (Option B)

**Goal:** Make klams self-populating and trustworthy as a data source, then
take a first hands-on read of TokenMaster against real data.

### Primary work (Option A)

1. **Run the systemd switchover for real.** Use the existing
   [deploy/install-systemd.sh](../../deploy/install-systemd.sh) (its
   `ENABLE_LIST` already covers `klams-scanner.timer` and
   `klams-monitor.service`) to install + enable the scanner timer and the Rust
   monitor on `kubs0`.
2. **Retire the legacy looper.** Stop and decommission
   `~/src/tools/ksvc-looper/klams_monitor.py` once the Rust monitor is
   confirmed emitting the same (typed) service lifecycle events.
3. **Verify ingestion end-to-end.** Confirm a scan cycle actually walks `~/src`
   and `~/obsidian`, chunks + embeds, and that new content becomes searchable
   within one cycle (the Phase 3 exit criterion in [plan.md](plan.md)). Spot-
   check knowledge memory is no longer empty.
4. **Fold in the two service bugs in this area:** kwi #32 (Authors
   `counts.writes` excludes knowledge) and kwi #33 (`bench-clean` Qdrant
   `?wait=true`). Both touch the service/ingestion path being exercised anyway.

### Spike (Option B) — TokenMaster "feel"

Timeboxed, exploratory, ships no production code. Validates the
[integration analysis](tokenmaster-integration/analysis.md):

- Run TokenMaster against a **Python repo** first (where graphify is proven) to
  see it at its best — not the klams Rust repo, where graphify's call graph is
  likely **sparse** (a documented TMX limitation for non-Python).
- Wire TMX's routing agent at the **klams MCP endpoint** per
  [analysis.md Option A](tokenmaster-integration/analysis.md) so the agent uses
  klams `memory_search` / `memory_add` as its durable/semantic layer — this is
  the actual integration seam, and it needs the ingestion work above to have
  real data to recall.
- Capture findings back into [tokenmaster-integration/](tokenmaster-integration/README.md)
  to inform whether "Lightweight graph memory" is worth pulling forward.

**Why this order:** Option A unblocks everything. The TokenMaster integration
thesis is "klams as TMX's semantic/temporal supplier" — that can't be evaluated
honestly against an empty memory store. Getting data flowing first makes the
spike meaningful and de-risks the deeper graph-memory direction. The primary
work is also low-risk: it's deployment + verification of already-tested code,
not new feature development.

---

## 3. Decision: migrate Spec Kit → ATV-StarterKit **after** this sprint

**Decision: do the ingestion sprint first under Spec Kit, then evaluate and
migrate to ATV at the sprint boundary.** This matches Ken's gut, and the
reasoning below backs it.

### What ATV is (and isn't)

[ATV-StarterKit](https://github.com/All-The-Vibes/ATV-StarterKit) is a one-
command installer that scaffolds a *broad* agentic Copilot environment: a
planning→build→review→ship→reflect pipeline (`/ce-brainstorm`, `/ce-plan`,
`/ce-work`, `/ce-review`, `/lfg`, `/slfg`), security auditing (`/atv-security`
— config + OWASP + STRIDE), session bookends (`/takeoff`, `/land`), a learning/
compound system, and 45+ skills / 51 agents across several upstream pillars
(Compound Engineering, gstack, autoresearch, agent-browser). It installs into
`.github/`, `.vscode/`, and `docs/`.

It is **not a strict 1:1 replacement for Spec Kit.** Spec Kit gives klams its
`specs/00X-*/` spec→plan→tasks→implement discipline and the `speckit.*` agents.
ATV's `/ce-plan` → `/ce-work` pipeline overlaps that planning core but is a
wider environment with a different folder/workflow model. Migrating is a
**substrate change**, not a tool swap.

> Worth noting: ATV-StarterKit is an All-The-Vibes project, and the TokenMaster
> author (`shyamsridhar123`) is a major contributor — so the TMX spike and an
> eventual ATV adoption sit in the same ecosystem.

### Why after, not during

1. **The sprint barely uses Spec Kit's strength.** This is deployment/ops + a
   spike, with light spec authoring. There's little upside to switching
   toolchains for it, and real cost: new commands, new conventions, retooled
   muscle memory mid-flight.
2. **Migrations belong at sprint boundaries.** klams has nine sprints of Spec
   Kit artifacts under `specs/`. Changing the planning substrate is structural
   and should not be entangled with shipping the ingestion fix.
3. **It deserves its own evaluation.** ATV touches `.github/`, `.vscode/`,
   `docs/`, and ships hooks + instructions templates. We need to decide what
   happens to the existing `specs/` tree, the `.specify/` constitution, and
   `.github/copilot-instructions.md` before committing — that's an evaluation
   task, not a side effect.
4. **Data-first makes the ATV evaluation better too.** With ingestion live,
   klams has real memory when we test ATV — and ATV's learning/compound layer
   is a future candidate to *write into* klams (a synergy to explore, not a
   commitment).

### Guardrails for the eventual switch

- **Trial on a scratch repo first.** Run `npx atv-starterkit@latest init`
  (ideally `--guided`) on a throwaway clone before touching klams, so the
  post-sprint switch isn't a leap of faith.
- **Audit what it clobbers.** Confirm how the installer treats the existing
  `.github/copilot-instructions.md`, `.specify/`, and `specs/`. ATV's
  uninstall claims checksum-based preservation of user-modified files and never
  touches `.vscode/`; verify that against our tree.
- **Decide the `specs/` fate explicitly.** Keep the historical Spec Kit specs
  as an archive vs. port forward — a named decision in the migration spec.

---

## 4. Summary

- **State:** 001–009 shipped; service is live on `kubs0`; **scanner + monitor
  are built but not deployed** (ingestion is effectively idle). Open: kwi #31,
  #32, #33.
- **Next sprint:** *Operationalize ingestion* (Option A) — switch scanner +
  monitor to systemd, retire the python looper, verify knowledge populates,
  close kwi #32 + #33 — with a **TokenMaster spike** (Option B) layered on top.
- **Toolchain:** **stay on Spec Kit for this sprint; evaluate and migrate to
  ATV-StarterKit at the next sprint boundary**, gated by a scratch-repo trial
  and a clobber audit.
