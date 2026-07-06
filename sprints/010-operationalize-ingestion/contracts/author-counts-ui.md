# Contract: Authors knowledge-count render (US4 / kwi #32)

**Feature**: `010-operationalize-ingestion`
**Scope**: viewport render only — the backend already supplies the value.

Per [../research.md](../research.md) §R1, the API returns
`AuthorCounts.knowledge` and the viewport `AuthorCounts` TS interface
already declares it. The only gap is that the Svelte pages render
`counts.writes` and never display `counts.knowledge`. This contract fixes
the render so facts and knowledge are both visible and distinguishable
(FR-015, FR-016).

## Data already available (no change)

```ts
// viewport/src/lib/types.ts — already present
interface AuthorCounts {
  writes: number;       // facts
  knowledge: number;    // knowledge_items  ← present, currently unrendered
  events: number;
  soft_deletes: number;
  restores_received: number;
}
```

## Render requirements

- **U1** The Authors **list** (`viewport/src/routes/authors/+page.svelte`)
  MUST display a `Knowledge` measure alongside the existing `Writes`
  column, bound to `a.counts.knowledge`.
- **U2** The Authors **detail**
  (`viewport/src/routes/authors/[id]/+page.svelte`) MUST surface
  `author.counts.knowledge` alongside `writes` so the summary line no
  longer implies writes are the author's only output.
- **U3** Facts (`writes`) and knowledge (`knowledge`) MUST be visually
  distinct (separate columns/labels), not summed into one number —
  preserving the two-kind distinction the backend already makes.

## Acceptance (vitest, written first)

| Test | Asserts | Maps to |
|------|---------|---------|
| list renders knowledge count | a row with `counts.knowledge = N` shows `N` in a distinct cell | U1, FR-015 |
| detail renders knowledge count | the detail summary shows the author's `knowledge` value | U2, FR-015 |
| facts vs knowledge distinct | writes and knowledge appear as separate values, not a sum | U3, FR-016 |
| author with only knowledge | an author with `writes=0, knowledge=N` shows `N`, not `0` | SC-008 |

## Out of scope

- No backend, store, or API change (already shipped sprint 009).
- No new endpoint or type field.
