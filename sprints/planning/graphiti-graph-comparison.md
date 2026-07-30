# graphiti vs klams — the temporal-graph comparator

**Date:** 2026-07-29
**Status:** Planning note. Companion to
[cognee-graph-comparison.md](cognee-graph-comparison.md); the same
gating experiment (§5 there) governs both. No work items created.
**Prompted by:** giving [graphiti](https://github.com/getzep/graphiti)
(Zep's open-source temporal knowledge graph) the same treatment as
cognee.
**Basis:** graphiti README, Zep docs (search), DeepWiki internals
summary, arXiv:2501.13956 abstract context, read 2026-07-29; klams at
`0.1.33` (`b3c4a17`).

---

## 0. The short version

Same bottom line as cognee — **don't adopt a graph store** — but
graphiti is the far more instructive comparator, for an unexpected
reason: **it independently converged on most of klams' architecture.**
LLM-free query path, RRF fusion, cross-encoder reranking, BM25 +
vector hybrid, contradiction handling by closing a validity window
instead of deleting — graphiti has all of these, and klams had all of
these before reading about graphiti. The comparison mostly *validates*
klams' design allocation rather than indicting it.

The genuine deltas are four: an entity graph with traversal-based
rerankers, a **bi-temporal model** (event time vs ingestion time,
point-in-time reads), *automatic* LLM-judged contradiction
invalidation at ingest, and community detection. Of these, the
bi-temporal distinction is the one cheap, borrowable idea (§4.1), and
the automatic-invalidation pipeline is — by the WI-259 division of
labor — literally a description of klams-mind's mandate (§4.3).

## 1. What graphiti is, and how it is not cognee

Graphiti builds a temporal knowledge graph over a real graph database
(Neo4j 5.26+, FalkorDB, Amazon Neptune) from **episodes** — discrete
raw inputs (a conversation turn, a JSON blob, a text passage). It
positions itself explicitly against GraphRAG-style batch systems, and
the differences from cognee are fundamental, not cosmetic:

| Axis | cognee | graphiti |
|---|---|---|
| Ingest unit | documents/chunks, batch "cognify" | **episodes**, incremental, no batch recompute |
| Intended corpus | document collections | **agent memory** — evolving, contradiction-prone streams |
| Time | not a first-class concern | **bi-temporal**: `created_at` (ingested), `valid_at` (true in the world), `invalid_at` (superseded), `expired_at` (versioning) |
| Contradictions | not addressed | new fact **closes the old edge's validity window**; nothing deleted, history stays queryable |
| Query time | flagship modes call an LLM to compose answers | **LLM-free**: hybrid retrieval + rerankers, sub-second |
| Ontology | RDF/OWL reference vocabulary | **Pydantic-prescribed entity types** — a small closed vocabulary in code |

Both still put an LLM at ingest: graphiti's pipeline per episode is
extract entities → resolve/dedupe against the existing graph →
extract relationship edges → invalidate contradicted edges, all via
structured-output LLM calls. The difference is the volume assumption:
episodes are small and agent-generated, not a re-scanned 180k-chunk
corpus.

Search is a recipe system over three methods — cosine similarity,
BM25, breadth-first graph traversal — combined by rerankers: **RRF**,
MMR (diversity), **cross-encoder** (BGE among the supported models),
node-distance (proximity to a focal entity), episode-mentions
(frequency). Results return edges (facts), nodes (entities), and
communities (label-propagation clusters with summaries).

## 2. Convergences — what graphiti and klams both concluded

Worth recording because two systems arriving at the same answers from
different directions is evidence the answers are right:

- **No LLM at query time.** Graphiti's stated reason (sub-second
  latency for agents) is klams' reason.
- **RRF over score mixing.** Graphiti fuses rank lists with RRF; klams
  moved to RRF in sprint 024 after raw-score mixing structurally
  favoured one source.
- **Cross-encoder rerank as a stage, BGE family.** Graphiti supports
  BGE cross-encoders; klams runs `bge-reranker-v2-m3` (sprint 030).
- **Supersede, don't delete.** Graphiti closes `invalid_at`; klams
  stamps `deleted_at` + `superseded_by` and keeps the tombstone
  restorable (sprint 029). Same shape, different names.
- **Hybrid lexical + semantic.** BM25 + embeddings there; Postgres
  FTS + Qdrant here.
- **Small prescribed vocabulary beats a public ontology.** Graphiti's
  Pydantic entity types are the same judgment the cognee note's §4.2
  made for a homelab entity list — and a second vote against OWL.

## 3. Genuine deltas — what graphiti has that klams doesn't

1. **The entity graph itself**, and the two rerankers that need it:
   node-distance (rank by proximity to a focal node) and BFS traversal
   as a retrieval method. klams' `memory_related` walks cosine space
   only.
2. **Bi-temporal reads.** klams records only ingest time.
   `volatility_demotion` runs on `created_at`, which for scanner
   content is *scan* time — a known approximation
   (architecture §2.5.7). There is no "what was true as of June?"
   query; superseded knowledge is admin-visible but not
   point-in-time searchable.
3. **Automatic contradiction invalidation.** Graphiti's LLM decides at
   ingest that a new fact contradicts an old edge and closes it,
   unattended. klams deliberately does *not*: trust-mismatch writes
   divert to `dissents` and an operator promotes or discards. One is
   optimized for unattended scale, the other for a single operator who
   wants no silent rewrites of canonical facts.
4. **Communities** — clustered entity groups with summaries, for
   corpus-level "what do I know about" questions. klams' extractive
   event summaries are the nearest (much smaller) analogue.

## 4. What to take

### 4.1 `occurred_at` — the cheap bi-temporal borrow (~small)

The event-time/ingest-time distinction does not need a graph. An
optional `occurred_at` on `memory_add`/fact writes (defaulting to
now) would let volatility demotion and fact decay run on when the
fact *became true* rather than when it was scanned — fixing the
acknowledged scan-time approximation for the curated stratum, where
authors actually know the real date. Payload/column + validator;
nothing downstream changes shape. File this one independently of any
graph evidence; it stands on its own.

Point-in-time *search* ("as of May") is the expensive half of
bi-temporality — skip it. Events + git history already reconstruct
the homelab timeline, and `memory_admin_list_deleted` +
`superseded_by` chains cover forensic digs.

### 4.2 Rerankers: nothing to do

MMR-diversity solves the redundancy problem klams already solves
structurally with content-hash duplicate collapse. Node-distance and
BFS need the graph (§3.1) and stay gated on the cognee note's §5
experiment. Episode-mentions reranking is a frequency prior — klams
already tracks `use_count` internally; resist wiring it into ranking
without eval evidence (popularity ≠ relevance in a 21-query gate).

### 4.3 Graphiti is a description of klams-mind's job

The sharpest takeaway. Graphiti's ingest loop — take an episode,
extract entities/claims, resolve against existing memory, supersede
what's contradicted — is, almost clause for clause, the WI-259
division of labor assigned to klams-mind ("background contradiction
detection and consolidation stay klams-mind's job — klams ships the
primitives"). klams' primitives are already sufficient for that loop:
`memory_search`/`similar_existing` for resolution,
`memory_supersede` for invalidation, `dissent_propose` for the
low-confidence path. If klams-mind ever grows the consolidation
pipeline, graphiti (MIT, Python, pluggable LLM) is worth reading —
or embedding — *there*, against the curated + extract strata only.
Its episode-scale cost model fits klams-mind's input volume exactly;
it was never going to fit the scanner's.

### 4.4 Still no graph database

Unchanged from the cognee note §3/§4.3: fifth stateful container,
three-store sync problem, no demonstrated relational-query misses,
and korg already holds the typed graph. Graphiti requiring Neo4j is
one of its heaviest costs even in its own community.

## 5. Decision posture

- The [cognee note's §5 gating experiment](cognee-graph-comparison.md)
  (mine `search_miss`/`search_sample` for relational-shaped failures;
  add multi-hop queries to the eval) remains the gate for anything
  graph-shaped. Nothing here changes it.
- §4.1 (`occurred_at`) and the cognee note's §4.1 (typed
  `related_ids` edges) are both small, evidence-independent, and
  compose: together they give klams most of graphiti's *semantics*
  (typed relations + real-world time + supersede lineage) with zero
  new infrastructure.
- Reassurance is a valid finding: klams' retrieval stack is not
  behind the state of the art here — it *is* the state of the art's
  query side, minus a graph whose necessity is unproven for this
  corpus and query load.

## 6. Sources

- [graphiti repository](https://github.com/getzep/graphiti)
- [graphiti docs](https://help.getzep.com/graphiti) — [searching](https://help.getzep.com/graphiti/working-with-data/searching)
- [Zep: A Temporal Knowledge Graph Architecture for Agent Memory (arXiv:2501.13956)](https://arxiv.org/html/2501.13956v1)
- [Neo4j blog: Graphiti — knowledge graph memory](https://neo4j.com/blog/developer/graphiti-knowledge-graph-memory/) (403 on fetch; not relied on)
- [DeepWiki: getzep/graphiti internals](https://deepwiki.com/getzep/graphiti)
- [Zep: What is a temporal knowledge graph?](https://www.getzep.com/ai-agents/temporal-knowledge-graph/)
