# cognee vs klams — do we want a knowledge graph?

**Date:** 2026-07-29
**Status:** Planning note. No work items created; §5 names the cheap
experiment that should precede any.
**Prompted by:** a survey of "things similar to klams" landing on
[cognee](https://docs.cognee.ai/core-concepts/overview), whose most
visible klams-doesn't-have-it feature is a knowledge graph.
**Basis:** cognee core-concepts docs read 2026-07-29 (overview,
architecture, DataPoints, ontologies, NodeSets, search operations);
klams at `0.1.33` (`b3c4a17`), `docs/architecture.md`.

---

## 0. The short version

cognee's graph is real and coherent, but it is bought with an LLM pass
over every ingested chunk — a cost model that fits neither klams'
corpus (~180k points, mostly source code, re-scanned hourly) nor two
standing klams decisions (no LLM in the service, sprint 032 #647;
YAGNI). klams already out-invests cognee at the retrieval end
(curated stratum, provenance tiers, cross-encoder rerank, RRF — none
of which cognee has an equivalent for), and it already stores
proto-graph edges it doesn't yet exploit.

Recommendation: **do not adopt a graph store or an extraction
pipeline.** Steal three cheap ideas (§4), and gate anything bigger on
evidence from the `search_miss` / `search_sample` logs (§5) — the
instrument that can actually tell us whether relational-shaped queries
fail today.

---

## 1. What cognee is

Three complementary stores:

- **Relational** — documents, chunks, provenance.
- **Vector** — embeddings for semantic similarity.
- **Graph** — entities and relationships as typed nodes/edges.

The graph is produced at ingest by **"cognify"**: an LLM
entity/relationship-extraction pass over every chunk. Optionally an
RDF/OWL **ontology** acts as a reference vocabulary so extracted
mentions resolve to canonical concepts ("kubs0" and "the kubs0
server" become one node); without it, node labels are whatever the
LLM emitted, inconsistently across documents.

Other notable concepts:

- **DataPoints** — Pydantic models that *are* the graph schema:
  scalar fields → node properties, DataPoint-typed fields → edges,
  `Embeddable()` fields → what gets vectorized. Structure-first,
  where klams is text-first with structured payload.
- **NodeSets** — tags materialized as first-class graph nodes with
  `belongs_to_set` edges, inherited by extracted entities; OR/AND
  filter semantics at search time.
- **Search types** (15+): the flagship is `GRAPH_COMPLETION` — vector
  search is only a *seed* that finds candidate triplets; the
  surrounding subgraph is expanded, formatted as
  "Nodes: … Connections: …" context, and handed to an LLM to compose
  the answer. Variants add multi-hop chain-of-thought traversal,
  iterative subgraph expansion, temporal ranking, raw Cypher, and
  NL→Cypher.

## 2. Feature-by-feature against klams

| cognee | klams | Verdict |
|---|---|---|
| Vector store + semantic search | Qdrant + the §2.5 pipeline: curated stratum, query-relative boost gate, three-tier provenance weights, cross-encoder rerank, weighted RRF | **klams ahead.** cognee has no equivalent of the boost gate, tier arbitration, or the eval gate |
| Relational / provenance store | Postgres facts, events, authors; payload provenance; dissents | Comparable |
| Entity/relationship graph | **Missing as a first-class layer.** Proto-edges exist unexploited: `supersedes`/`superseded_by`, dissents' `contradicting_memory_id`, `copies[]` cross-host identity, `author_id`, repo/file/machine payload structure, tags | The gap this note is about |
| Graph traversal at query time | `memory_related` walks by cosine only — it never follows a typed edge | Real gap, cheap to close partially (§4.1) |
| Ontology / entity canonicalization | None — "kubs0" in a fact and in a scanned chunk are unrelated strings | Gap; belongs in klams-mind if anywhere (§4.2) |
| NodeSets | Tags (AND-all post-filter) | Functionally equivalent at this scale; OR semantics trivial if ever wanted |
| Temporal search | `event_search`, `since`/`until` filters, extractive summaries | Covered |
| LLM at ingest | Deliberately none in the service; extraction is klams-mind's job (WI-259 division of labor) | Standing decision, reaffirmed here |

The structural observation: **cognee spends its sophistication at
ingest and keeps retrieval simple; klams spends it at retrieval and
keeps ingest deterministic.** For a single-operator system where the
scanner runs hourly unattended and retrieval changes are gated by a
21-query eval, klams' allocation is the right one. cognee's design
assumes LLM calls on every document are affordable and that the query
load is relational. Neither holds here yet.

## 3. Why not adopt the graph

1. **Ingest cost.** An LLM extraction pass over ~180k points, repeated
   for every changed file every hour, is exactly the cost profile
   klams was built to avoid. And LLM triplet extraction is at its
   weakest on source code, which is most of the corpus.
2. **Contradicts standing decisions.** Sprint 032 removed the LLM
   client from the service on the evidence that no code path ever
   used it; AGENTS.md's YAGNI rule requires new complexity to justify
   itself. A graph store (Neo4j/FalkorDB/…) is a fifth stateful
   container plus a query language plus a sync problem between three
   stores.
3. **No demonstrated need.** The graph pays off for relational,
   multi-hop questions ("which services depend on X across which
   machines"). The eval suite is 21 recall-shaped queries at 21/21.
   Whether relational queries *fail today* is an empirical question
   the miss log can answer (§5) — no evidence yet says they do.
4. **A typed-node graph already exists in the homelab: korg.** If an
   entity graph ever earns its keep, the first question is whether it
   belongs there, not whether to stand up a second graph system.

## 4. What to steal (ascending cost)

### 4.1 Typed edges between agent memories (~small, no LLM)

klams already stores one typed edge (`supersedes`). Generalize
slightly:

- `memory_add` / `memory_update` accept optional `related_ids`,
  stored in point payload — agents already know the related memory at
  write time (the `similar_existing` probe hands it to them).
- `memory_related` returns the **edge neighborhood** alongside the
  cosine neighbors: supersede chain, dissent links, explicit
  relations, same-repo siblings, `copies[]`.

This is cognee's "structured context assembly" built entirely from
edges klams already has or can accept for free. No new store, no
schema migration beyond payload fields, degrades to today's behavior
when the fields are absent.

### 4.2 A homelab entity vocabulary — in klams-mind, not klams (~medium, gated on §5)

cognee's ontology idea shrinks beautifully at homelab scale: the
entity set is enumerable (machines, services, repos). klams-mind's
extraction pipeline could stamp its extracts with canonical
`entities: ["kubs0", "qdrant", …]` in payload, keyword-indexed the
way `machines[]` already is. That yields entity-filtered retrieval
and a poor-man's "everything about X" traversal for the ~100-point
curated stratum — the only stratum where it matters — while
respecting the WI-259 split: klams ships primitives, klams-mind does
the smart extraction. klams' side is one payload field plus one
keyword index.

### 4.3 A graph database (~large) — no

Rejected per §3. Revisit only if §5 produces evidence *and* §4.1/§4.2
prove insufficient *and* the korg-hosting question has been answered.

## 5. The gating experiment (do this first)

Before creating any work item from §4.2 or beyond:

1. **Grep the miss log for relational-shaped queries.** `search_miss`
   and `search_sample` hold every real query with its top raw score.
   Look for multi-hop / relationship-shaped phrasings that missed or
   scored weak.
2. **Add 2–3 deliberately multi-hop queries to the eval suite**
   (klams-mind, `just eval`) — e.g. a query whose answer requires
   joining a fact about a machine with knowledge about a service.
3. If the current pipeline passes them, close this topic. If it
   fails, the failures define the scope — and §4.1 is the first
   remedy to try, being nearly free.

§4.1 is worth doing on its own merits regardless of the experiment's
outcome; it needs no graph evidence, only a sprint slot.

## 6. Sources

- [cognee core concepts — overview](https://docs.cognee.ai/core-concepts/overview)
- [cognee architecture](https://docs.cognee.ai/core-concepts/architecture)
- [DataPoints](https://docs.cognee.ai/core-concepts/building-blocks/datapoints)
- [Ontologies](https://docs.cognee.ai/core-concepts/further-concepts/ontologies)
- [NodeSets](https://docs.cognee.ai/core-concepts/further-concepts/node-sets)
- [Search operations](https://docs.cognee.ai/core-concepts/main-operations/search)
