// Sprint 010 US4 (kwi #32) — Acceptance tests for the Authors **list**
// knowledge-count render (U1 / FR-015 / FR-016). The list renders its
// count cells from `authorCountCells`, so asserting the helper output
// asserts what the list row displays. No DOM testing-library is
// configured (see sibling `[id]/row.test.ts`); these are pure-data
// acceptance tests against the shared render helper.

import { describe, expect, it } from 'vitest';
import type { AuthorCounts } from '$lib/types';
import { authorCountCells } from './counts';

const counts: AuthorCounts = {
  writes: 3,
  knowledge: 42,
  events: 7,
  soft_deletes: 1,
  restores_received: 0
};

describe('Authors list renders knowledge count (U1, FR-015)', () => {
  it('renders counts.knowledge in a distinct cell', () => {
    const cells = authorCountCells(counts);
    const knowledge = cells.find((c) => c.key === 'knowledge');
    expect(knowledge).toBeDefined();
    expect(knowledge?.value).toBe(42);
    expect(knowledge?.label).toBe('Knowledge');
  });

  it('keeps writes (facts) and knowledge as separate values, not a sum (U3, FR-016)', () => {
    const cells = authorCountCells(counts);
    const writes = cells.find((c) => c.key === 'writes');
    const knowledge = cells.find((c) => c.key === 'knowledge');
    // Distinct cells with distinct labels.
    expect(writes?.value).toBe(3);
    expect(knowledge?.value).toBe(42);
    expect(writes?.label).not.toBe(knowledge?.label);
    // No cell carries the summed value 45.
    expect(cells.some((c) => c.value === counts.writes + counts.knowledge)).toBe(false);
  });
});
