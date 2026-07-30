# docs/sharing — knowledge leaving the homelab

> **⚠️ THOUGHT STAGE.** Everything in this directory is brainstorm and
> design work — **no build commitment has been made**. It records
> serious thinking (sprint 038, korg:772) about whether and how klams
> memories could be shared beyond this homelab, so that a future
> implementation sprint starts from decisions instead of vibes. If
> you are looking for how klams *works today*, this is the wrong
> directory — see [docs/architecture.md](../architecture.md).

This directory lives outside `sprints/` deliberately: sprint dirs are
frozen chronicles of what happened; this is a living design space that
later sprints are expected to revise in place.

## The idea in one paragraph

Two brainstorms — a **community shared store** (korg #760) and
**curated exfil digests** (korg #768) — turn out to be one design:
each operator's agents curate, sanitize, and publish their hardest-won
memories as a small **signed static feed**; peers subscribe, and their
own memory systems ingest the feeds into a locally-ranked community
stratum. Not a general knowledge store — the Internet exists — but a
share among people whose interests overlap: *the hard problems Alice
has solved are likely related to the hard problems Bob will
encounter.*

## The pitch

**[▶ Rendered infographic](https://htmlpreview.github.io/?https://github.com/kenhia/klams/blob/main/docs/sharing/pitch.html)**
(via htmlpreview.github.io; source: [pitch.html](pitch.html) —
self-contained, no external requests, light/dark aware).

## Documents

| Document | What it is |
|---|---|
| [design.md](design.md) | The design/decision record: why publication beats a shared hub, query-time composition, the five export gates, auth, threat model, go/no-go with named triggers. |
| [prior-art.md](prior-art.md) | Research findings (2026-07-30): the interchange-standards survey and its verdict, memory-poisoning literature, sanitization tooling, hosting costs, syndication/signing precedents. All sourced. |
| [memory-feed-draft.md](memory-feed-draft.md) | **Memory Feed v0.1-draft** — a proposed minimal interchange format (markdown + YAML frontmatter, signed manifest) written system-neutral, because the survey found no adopted standard to use instead. |
| [pitch.html](pitch.html) | The infographic above. |

## Status and next steps

The design's own conclusion (design.md §10): the format draft, an
exporter skill, and an import skill are worth building at N=1; the
community machinery is **triggered, not scheduled** — the named
trigger is a second real operator (the 035 Operator track's install
path) who wants to trade digests. Nothing here is on the sprint queue
until Ken promotes it; when that happens, design.md §10's build list
is the seed for the work items.
