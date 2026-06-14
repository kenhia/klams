// Sprint 010 US4 (kwi #32) — Acceptance tests for the Authors **detail**
// knowledge-count render (U2 / FR-015 / SC-008). The detail summary
// renders its measures from `authorCountCells`, so asserting the helper
// output asserts what the summary line displays. No DOM testing-library
// is configured (see sibling `row.test.ts`); these are pure-data
// acceptance tests against the shared render helper.

import { describe, expect, it } from 'vitest';
import type { AuthorCounts } from '$lib/types';
import { authorCountCells } from '../counts';

describe('Authors detail surfaces knowledge alongside writes (U2, FR-015)', () => {
  it('includes both writes and knowledge measures in the summary', () => {
    const counts: AuthorCounts = {
      writes: 5,
      knowledge: 12,
      events: 0,
      soft_deletes: 0,
      restores_received: 0
    };
    const cells = authorCountCells(counts);
    const labels = cells.map((c) => c.label);
    expect(labels).toContain('Writes');
    expect(labels).toContain('Knowledge');
    expect(cells.find((c) => c.key === 'writes')?.value).toBe(5);
    expect(cells.find((c) => c.key === 'knowledge')?.value).toBe(12);
  });

  it('an author with writes=0, knowledge=N shows N not 0 (SC-008)', () => {
    const counts: AuthorCounts = {
      writes: 0,
      knowledge: 9,
      events: 0,
      soft_deletes: 0,
      restores_received: 0
    };
    const cells = authorCountCells(counts);
    expect(cells.find((c) => c.key === 'knowledge')?.value).toBe(9);
    expect(cells.find((c) => c.key === 'writes')?.value).toBe(0);
  });
});
