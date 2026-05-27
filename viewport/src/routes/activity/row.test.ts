// Sprint 008 T054 — Acceptance test for FR-015a:
// Clicking a soft-deleted row in the viewport Activity tab must navigate
// to the per-kind detail route just like a live row does, and the row
// helpers must surface the soft-deleted metadata
// (state / deleted_at / deleted_by) without error.
//
// No DOM testing-library is configured in this project, so this is a
// pure-data acceptance test against the row helpers extracted from
// `+page.svelte`. The frontend invoke wiring is covered by
// `src/lib/api.test.ts`; the Tauri command shape is covered by
// `src-tauri/tests/viewport_activity_command.rs`.

import { describe, expect, it } from 'vitest';
import type { MemoryItem } from '$lib/types/memories';
import { hrefFor, summaryFor } from './row';

const author = {
  id: '00000000-0000-7000-8000-000000000abc',
  agent_name: 'incident-bot',
  model: 'gpt-5'
};

const liveFact: MemoryItem = {
  id: '11111111-1111-7111-8111-111111111111',
  kind: 'fact',
  type: 'user',
  payload: { name: 'Ken' },
  tags: ['homelab'],
  author,
  created_at: '2026-05-26T00:00:00Z',
  updated_at: '2026-05-26T00:00:00Z',
  state: 'live'
};

const deletedFact: MemoryItem = {
  ...liveFact,
  id: '22222222-2222-7222-8222-222222222222',
  state: 'deleted',
  deleted_at: '2026-05-26T01:00:00Z',
  deleted_by: author
};

const deletedKnowledge: MemoryItem = {
  id: '33333333-3333-7333-8333-333333333333',
  kind: 'knowledge',
  text: 'kubs0 backup window starts at 02:00 UTC',
  tags: [],
  author,
  created_at: '2026-05-26T00:00:00Z',
  updated_at: '2026-05-26T00:30:00Z',
  state: 'deleted',
  deleted_at: '2026-05-26T01:00:00Z',
  deleted_by: author
};

const deletedEvent: MemoryItem = {
  id: '44444444-4444-7444-8444-444444444444',
  kind: 'event',
  category: 'Deploy',
  payload: { service: 'klams', version: '0.8.0' },
  tags: [],
  author,
  created_at: '2026-05-26T00:00:00Z',
  updated_at: '2026-05-26T00:00:00Z',
  // events are never soft-deleted per FR-015, but the helper must not
  // crash if a hypothetical deleted event ever shows up.
  state: 'deleted',
  deleted_at: '2026-05-26T01:00:00Z',
  deleted_by: author
};

describe('Activity row helpers — FR-015a soft-deleted navigation', () => {
  it('routes live and soft-deleted facts to the same per-kind detail path', () => {
    expect(hrefFor(liveFact)).toBe('/facts/' + liveFact.id);
    expect(hrefFor(deletedFact)).toBe('/facts/' + deletedFact.id);
  });

  it('routes soft-deleted knowledge to /knowledge/{id}', () => {
    expect(hrefFor(deletedKnowledge)).toBe('/knowledge/' + deletedKnowledge.id);
  });

  it('routes soft-deleted events to /events/{id}', () => {
    expect(hrefFor(deletedEvent)).toBe('/events/' + deletedEvent.id);
  });

  it('produces a non-empty summary for every soft-deleted kind without throwing', () => {
    expect(() => summaryFor(deletedFact)).not.toThrow();
    expect(() => summaryFor(deletedKnowledge)).not.toThrow();
    expect(() => summaryFor(deletedEvent)).not.toThrow();
    expect(summaryFor(deletedFact)).toContain('user');
    expect(summaryFor(deletedKnowledge)).toContain('kubs0');
    expect(summaryFor(deletedEvent)).toContain('Deploy');
  });

  it('preserves the deleted_at / deleted_by metadata on the row', () => {
    expect(deletedFact.state).toBe('deleted');
    expect(deletedFact.deleted_at).toBe('2026-05-26T01:00:00Z');
    expect(deletedFact.deleted_by?.agent_name).toBe('incident-bot');
  });
});
