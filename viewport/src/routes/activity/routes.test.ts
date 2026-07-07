// Sprint 015 (kwi #31) — every hrefFor() target must have a real
// SvelteKit route. The detail links have existed since sprint 008 but
// the pages were never created, so /facts/{id}, /events/{id}, and
// /knowledge/{id} 404'd. This guards against the next dangling link.
import { describe, expect, it } from 'vitest';

const pages = Object.keys(import.meta.glob('/src/routes/**/+page.svelte'));

describe('hrefFor targets have routes', () => {
  it.each([
    ['fact', '/src/routes/facts/[id]/+page.svelte'],
    ['event', '/src/routes/events/[id]/+page.svelte'],
    ['knowledge', '/src/routes/knowledge/[id]/+page.svelte']
  ])('%s detail route exists', (_kind, route) => {
    expect(pages).toContain(route);
  });
});
