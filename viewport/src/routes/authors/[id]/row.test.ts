// Sprint 009 T031 (US4) — Acceptance test for FR-017 / FR-018:
// the Authors view memory rows must use the same `hrefFor()` link
// builder as the Activity view so clicking a Summary cell reaches
// the per-kind detail route on the first click (closes kwi #28).
//
// No DOM testing-library is configured; this is a pure-data
// acceptance test that imports the shared row helpers and asserts
// they produce the expected URLs for an `AuthorMemoryRow`-shaped
// input (structurally a `MemoryItem`).

import { describe, expect, it } from 'vitest';
import type { MemoryItem } from '$lib/types/memories';
import { hrefFor, summaryFor } from '../../activity/row';

const author = {
  id: '00000000-0000-7000-8000-000000000abc',
  agent_name: 'incident-bot',
  model: 'gpt-5'
};

const factRow: MemoryItem = {
  id: '11111111-1111-7111-8111-111111111111',
  kind: 'fact',
  type: 'user',
  payload: { name: 'Ken' },
  tags: [],
  author,
  created_at: '2026-05-26T00:00:00Z',
  updated_at: '2026-05-26T00:00:00Z',
  state: 'live'
};

const knowledgeRow: MemoryItem = {
  id: '22222222-2222-7222-8222-222222222222',
  kind: 'knowledge',
  text: 'kubs0 backup window starts at 02:00 UTC',
  tags: [],
  author,
  created_at: '2026-05-26T00:00:00Z',
  updated_at: '2026-05-26T00:00:00Z',
  state: 'live'
};

const eventRow: MemoryItem = {
  id: '33333333-3333-7333-8333-333333333333',
  kind: 'event',
  category: 'Deploy',
  payload: { service: 'klams' },
  tags: [],
  author,
  created_at: '2026-05-26T00:00:00Z',
  updated_at: '2026-05-26T00:00:00Z',
  state: 'live'
};

describe('authors view row link parity (FR-017/FR-018)', () => {
  it('fact rows link to /facts/:id', () => {
    expect(hrefFor(factRow)).toBe(`/facts/${factRow.id}`);
  });

  it('knowledge rows link to /knowledge/:id', () => {
    expect(hrefFor(knowledgeRow)).toBe(`/knowledge/${knowledgeRow.id}`);
  });

  it('event rows link to /events/:id', () => {
    expect(hrefFor(eventRow)).toBe(`/events/${eventRow.id}`);
  });

  it('summary builders are non-empty for all kinds', () => {
    expect(summaryFor(factRow)).toContain('user');
    expect(summaryFor(knowledgeRow)).toContain('kubs0');
    expect(summaryFor(eventRow)).toContain('Deploy');
  });
});
