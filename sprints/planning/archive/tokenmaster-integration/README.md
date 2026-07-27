# TokenMaster × klams — integration analysis

**Status:** Exploratory analysis (no commitment)  
**Date:** 2026-06-09  
**Author:** Ken (with GHCP)  
**Subject repo:** [`shyamsridhar123/TokenMasterX`](https://github.com/shyamsridhar123/TokenMasterX) (local clone: `/home/ken/src/opc/TokenMasterX`)  
**Requested by:** TokenMaster's author, to scope how the tool could integrate with klams.

## What this folder is

The author of TokenMaster asked how their tool could plug into klams. This
folder is a written-up review of TokenMaster, a side-by-side of the two
systems, and a set of integration options — split cleanly into things we
could do **today against the systems as they stand** and **future-looking**
changes that would require new work in klams, TokenMaster, or both.

Nothing here is scheduled. It is decision input.

## Documents

| File | Contents |
|------|----------|
| [analysis.md](analysis.md) | What TokenMaster is, what klams is, why they are complementary rather than competing, and concrete near-term integration options that work against both systems as they exist today. |
| [future-synergies.md](future-synergies.md) | Future-looking synergies that require new features in klams and/or TokenMaster, with the load-bearing changes called out explicitly. |

## One-paragraph summary

TokenMaster and klams solve **different halves of the same problem** —
"stop the agent from re-deriving what it already knows." TokenMaster owns the
**structural / spatial** half (a per-repo code graph that answers
callers / callees / impact / inheritors, enforced by a routing agent so the
model *can't* default to grep) and optimizes **per-session token economics**.
klams owns the **semantic + durable + cross-session + multi-agent** half (facts,
events, embedded knowledge, attribution, decay) exposed over a persistent MCP
server. TokenMaster explicitly punts on durable cross-session memory — it
borrows the host CLI's lexical session store as a stopgap "temporal layer."
That gap is exactly what klams already is. The cleanest integration is to let
TokenMaster's routing agent use **klams as its temporal/semantic memory
supplier**, and — further out — to let **klams host the structural code graph
itself** (which is already on the klams backlog as "Lightweight graph memory"),
giving one durable store that answers structural, semantic, and temporal
questions for every agent and machine, not one CLI home.
