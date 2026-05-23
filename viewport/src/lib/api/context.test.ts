/** Vitest specs for the `/memory/context` typed client.
 *
 *  Asserts the args envelope, response decoding (including
 *  degraded sections), and surfacing of 503 + `Retry-After`. */

import { describe, it, expect, vi, beforeEach } from 'vitest';

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args)
}));

import { contextApi, type ContextBundle, type ContextRequest } from './context';

beforeEach(() => {
  invokeMock.mockReset();
});

describe('contextApi.fetch', () => {
  it('encodes the request under {args}', async () => {
    invokeMock.mockResolvedValueOnce({
      facts: [],
      knowledge: [],
      events: [],
      total_spent: 0,
      truncated: false,
      token_encoder: 'cl100k_base',
      sections: {}
    } satisfies ContextBundle);

    const req: ContextRequest = {
      query: 'gpu drivers on kai',
      token_budget: 2048,
      filters: { host: 'kai', type: 'EnvFact' }
    };
    await contextApi.fetch(req);

    expect(invokeMock).toHaveBeenCalledWith('memory_context', { args: req });
  });

  it('decodes a degraded section', async () => {
    invokeMock.mockResolvedValueOnce({
      facts: [],
      knowledge: [],
      events: [],
      total_spent: 0,
      truncated: false,
      token_encoder: 'cl100k_base',
      sections: {
        knowledge: {
          count: 0,
          tokens_spent: 0,
          source: 'raw',
          status: 'degraded',
          degraded_reason: 'qdrant unavailable'
        }
      }
    } satisfies ContextBundle);

    const bundle = await contextApi.fetch({ query: 'x', token_budget: 128 });
    expect(bundle.sections.knowledge.status).toBe('degraded');
    expect(bundle.sections.knowledge.degraded_reason).toBe('qdrant unavailable');
  });

  it('surfaces 503 ViewportError with retry hint', async () => {
    invokeMock.mockRejectedValueOnce({
      kind: 'server',
      status: 503,
      message: 'all retrieval sources unavailable (retry_after_seconds=5)'
    });

    await expect(contextApi.fetch({ query: 'x', token_budget: 64 })).rejects.toMatchObject({
      kind: 'server',
      status: 503,
      message: expect.stringContaining('retry_after_seconds=5')
    });
  });
});
