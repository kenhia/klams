// Sprint 010 US4 (kwi #32) — shared, testable rendering of the
// per-author count measures. Both the Authors list and detail pages
// render from `authorCountCells` so `writes` (facts) and `knowledge`
// stay separate, distinctly-labelled measures and are never summed
// into one number (FR-015, FR-016, U1/U2/U3). Kept as a pure helper
// because the viewport has no DOM testing-library — see the sibling
// `[id]/row.test.ts` for the same pure-data acceptance-test pattern.

import type { AuthorCounts } from '$lib/types';

export interface CountCell {
  key: keyof AuthorCounts;
  label: string;
  value: number;
}

/** Ordered measures shown for an author. `writes` and `knowledge` are
 * deliberately separate cells (U3) — facts and knowledge are distinct
 * outputs the backend already counts independently. */
export function authorCountCells(counts: AuthorCounts): CountCell[] {
  return [
    { key: 'writes', label: 'Writes', value: counts.writes },
    { key: 'knowledge', label: 'Knowledge', value: counts.knowledge },
    { key: 'events', label: 'Events', value: counts.events },
    { key: 'soft_deletes', label: 'Soft deletes', value: counts.soft_deletes },
    { key: 'restores_received', label: 'Restores', value: counts.restores_received }
  ];
}
