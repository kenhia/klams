# Sprint 041 — Tag-filtered search: push the filter down to Qdrant

**Proposal:** korg:815 (covers #799)
**Started:** 2026-07-30 · **Version:** 0.1.41
**Type:** retrieval correctness. Behaviour change in the shared pipeline.

## Goal

`memory_search` with a `tags` argument returns a starved page. Make the
ANN search run **over** the tagged subset instead of pruning a pool it
has already fetched.

## The live baseline — the WI's number is stale, and the truth is worse

The proposal quotes "returns **1 hit**", measured during sprint 037.
Re-measured on deployed **0.1.40** before any code change, against a
`gotcha` stratum of 36 points, `top_k: 10`:

| Query (`tags: ["gotcha"]`) | Hits |
|---|---|
| `"klams gotcha"` | **4** |
| `"port binding"` | **1** |
| `"deployment surprises to watch out for"` | **0** |

Two things follow, and they sharpen the case rather than weaken it:

1. **Sprint 037's lexical list masks the bug** wherever query tokens
   appear literally in tagged text. That is why "klams gotcha" now
   returns 4 rather than the filed 1 — the lexical arm feeds in
   candidates that happen to survive the retain. It is luck, not
   design.
2. **The real floor is 0, not 1.** On ordinary prose queries the
   lexical arm is empty (it requires *every* token present), so the
   result is pure global-ANN + retain — and the third query above
   returns nothing at all, from a stratum that visibly contains
   matching material (the tailscale and port-conflict gotchas).

So the failure is worst exactly where semantic search is supposed to
earn its keep, and invisible wherever literal overlap happens to rescue
it. A convention that works only when you already know the words is not
a convention.

Corpus state at baseline: `knowledge_items_v2` holds 180,889 points, of
which **133** carry any `tags` payload at all. `tags` is a **keyword
payload index** on the live collection — verified before starting,
because the whole fix rests on the pushdown being an indexed filter
rather than a scan.

## What the WI missed: there are two tag paths, not one

`RetrievalFilters` already carries a `tag: Option<String>`, and it is a
different mechanism from `SearchParams.tags: Option<Vec<String>>`:

| | `filters.tag` (singular) | `params.tags` (plural) |
|---|---|---|
| Set by | REST `/memory/search` `filters` | MCP `memory_search` only (REST passes `None`) |
| Applied | `knowledge_matches_filters`, per-arm, post-fetch | one `retain` after all arms |
| Widens the fetch | **yes** — counts in `filters_active`, so `FILTER_OVERFETCH` fires | **no** |

Two mechanisms for one concept, with different behaviour, and only one
of them widens. The WI saw the second and diagnosed it correctly; it did
not know the first existed. This sprint fixes the starvation and makes
the two agree on fetch widening, but does **not** merge them — that is a
larger contract change across both surfaces and wants its own decision.

## Approach

1. **One new `Store` method**, `search_knowledge_tagged`, with a default
   impl that errors like `search_knowledge_curated` /
   `search_knowledge_lexical` already do. Deliberately *not* three
   changed signatures: `search_knowledge` alone has 17 implementations
   across the crates and their mocks, and widening all of them to carry
   a parameter most of them ignore is churn without value.
2. **The global ANN arm uses it when tags are present.** Running an
   unfiltered ANN under an active tag filter is pure waste — every
   surviving result has to carry the tags anyway — so this replaces that
   arm rather than adding to it.
3. **Curated and lexical stay as they are**, retained post-fetch. They
   are small by construction (the curated stratum is ~100 points;
   the lexical list requires every query token present), so neither can
   starve the way a 180k-point global ANN does. If the live re-measure
   shows otherwise, that is visible and becomes a follow-up rather than
   a guess made now.
4. **The post-projection retain stays**, as the backstop and as the
   authority for facts and events, whose tags live in Postgres. For
   knowledge under pushdown it becomes a no-op, which is the point: the
   filter's semantics are defined in one place and the pushdown is an
   optimisation that must agree with it.
5. **`filters_active` counts tags**, so the facts/events arm widens its
   fetch the way an equivalent `filters.tag` already does.

Multi-tag semantics stay AND (`must` over one `Condition::matches` per
tag), matching the retain's `all()`.

## Acceptance

- The three baseline queries measurably improve on the live corpus, and
  the prose query in particular stops returning nothing.
- A regression test that starves under the old path and passes under the
  new one.
- Facts/events tag filtering unchanged.
- `just gate` and the integration suite green.

## Ship checklist

- [ ] **Restart `klams-scanner.timer`** — stopped at sprint start for
      measurement hygiene (the scanner re-indexes this very sprint doc,
      and a doc about tags and gotchas would feed its own query terms
      back into the corpus being measured as *untagged* knowledge). A
      stopped scanner is silent: nothing alerts, `/healthz` stays green,
      the corpus just quietly stops tracking the filesystem.
      `sudo systemctl start klams-scanner.timer`

## Outcome

_(filled in at ship time)_
