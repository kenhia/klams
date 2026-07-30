# Sprint 038 — Sharing design: community store + curated exfil digests

**Proposal:** korg:772 (covers #760 Community shared `klams` store,
#768 Curated exfil digests)
**Started:** 2026-07-30 · **Version:** 0.1.38 (docs-only sprint; the
bump keeps the PATCH-=-sprint convention intact)
**Type:** design/decision sprint — no service code changes.

## Goal

Take Ken's two sharing brainstorms seriously as **two halves of one
design space** — knowledge leaving the homelab — and produce a durable
design record that:

1. settles the questions #760 posed (costs, implementation shape, auth
   rules, query-time composition with the primary-local instance);
2. picks the interchange/sanitization story shared by both ideas —
   the digest format is the natural wire format for the shared store;
3. researches whether an **existing standard** for cross-system
   agentic-memory interchange exists, and if not, drafts one;
4. reconciles with the adjacent planning notes (generalize-klams §4.1
   Operator track — an installable klams is the precondition for
   "would anyone share into it"; the cognee/graphiti notes stay gated
   on their own experiment);
5. ends with a go/no-go and, if go, the concrete WIs for a build
   sprint. "Not yet, here's the trigger" is a legitimate outcome.

## Scope decisions (from Ken's start-sprint input)

- Output lives in **`docs/sharing/`** — deliberately *outside*
  `sprints/`, because this is a living thought-stage document set, not
  a sprint chronicle. Its README states the thought-stage status
  loudly.
- If research surfaces no existing interchange standard, we write a
  **draft standard in its own document** — written so *other* agentic
  memory systems could adopt it, not as a klams internal format.
- A **self-contained HTML pitch infographic** for the brainstorm, with
  a rendered link in the README.
- Context from Ken: the klams backlog is nearly empty (742/viewport
  work is mostly *removal* now that `../klams-view` replaced the
  viewport), so this design should be written expecting implementation
  could start soon — including identifying which cloud service would
  host a real (non-simulated) shared store.

## Acceptance

- `docs/sharing/README.md` — thought-stage banner, document map,
  rendered pitch link.
- `docs/sharing/design.md` — the design/decision record (a)–(e) above.
- `docs/sharing/prior-art.md` — research findings with sources:
  existing standards (or their absence), comparable systems,
  sanitization tooling, hosting shapes + costs.
- Draft interchange standard document (only if no existing standard).
- `docs/sharing/pitch.html` — self-contained, no external requests.
- `just gate` passes (version bump touches Cargo.toml).

## Chronicle

- 2026-07-30 — Sprint opened. Research fan-out launched (existing
  interchange standards; shared-memory prior art + hosting costs).
  Local recon: generalize-klams.md §4.1/§4.2/§4.6, docs/auth.md, the
  036/037 retrieval pipeline. Early design insight recorded: since 036
  the retrieval pipeline is one `klams_core::retrieval::search` with
  five rank lists entering RRF fusion — a shared store composes most
  naturally as a *sixth rank list* (with its own provenance tier and
  the "keep X% from local" knob expressed as a fusion weight/quota),
  not as a client-side two-query merge.
- 2026-07-30 — Research back; the two headline verdicts: (1) **no
  adopted cross-system memory interchange standard exists** — only a
  May–July 2026 land-grab of one-author drafts (two colliding "PAM"s,
  two colliding "AMP"s, a weeks-old W3C CG with nothing published), so
  the draft standard was warranted and written; (2) **nobody has
  shipped a cross-operator community memory pool** — team-scope
  Hivemind is the closest and its trust story is bare RBAC. Ken added
  the framing that became the pitch's spine mid-sprint: this is *not*
  a general knowledge store (the Internet exists) — it's a share among
  overlapping-interest peers; Alice's solved hard problems predict
  Bob's future ones.
- 2026-07-30 — Design settled on **publication, not access**: the
  digest (#768) is the primitive; the shared store (#760) is a view
  over subscribed digests. Four topologies weighed (multi-writer hub,
  live federation, signed static feeds, hosted index); signed static
  feeds chosen — it is the only shape needing no tenancy model and no
  new klams auth surface, and it makes moderation/revocation/loop
  prevention structural. v0 needs ~zero klams-service changes; the
  eventual `community` provenance tier + page quota is eval-gated and
  triggered, not scheduled. Deliverables shipped: `docs/sharing/`
  (README, design.md, prior-art.md, memory-feed-draft.md — "Memory
  Feed v0.1-draft", markdown + YAML frontmatter + minisign-signed
  manifest), pitch.html verified rendering in light and dark.
- Go/no-go outcome: **partial go** — format draft + exporter skill +
  import skill are worth building at N=1; community machinery waits on
  a second real operator (trigger inherits from the 035 Operator
  track). Multi-writer anything: never.
