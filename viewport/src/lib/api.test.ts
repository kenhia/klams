/** Vitest specs for the `invoke()` wrappers in [`api.ts`](./api.ts).
 *
 *  We mock `@tauri-apps/api/core` so each test asserts the command
 *  name and `{args: {...}}` envelope passed to `invoke`. */

import { describe, it, expect, vi, beforeEach } from 'vitest';

const invokeMock = vi.fn();

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args)
}));

import { api } from './api';

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue({});
});

describe('api wrappers', () => {
  it('listFacts passes args under {args}', async () => {
    await api.listFacts({ fact_type: 'UserFact', limit: 25 });
    expect(invokeMock).toHaveBeenCalledWith('list_facts', {
      args: { fact_type: 'UserFact', limit: 25 }
    });
  });

  it('listFacts defaults args to empty object', async () => {
    await api.listFacts();
    expect(invokeMock).toHaveBeenCalledWith('list_facts', { args: {} });
  });

  it('listEvents wraps args', async () => {
    await api.listEvents({ category: 'agent.activity' });
    expect(invokeMock).toHaveBeenCalledWith('list_events', {
      args: { category: 'agent.activity' }
    });
  });

  it('searchUnified wraps query payload', async () => {
    await api.searchUnified({ query: 'hello', top_k: 5 });
    expect(invokeMock).toHaveBeenCalledWith('search_unified', {
      args: { query: 'hello', top_k: 5 }
    });
  });

  it('searchKnowledge calls the dedicated command', async () => {
    await api.searchKnowledge({ query: 'k' });
    expect(invokeMock).toHaveBeenCalledWith('search_knowledge', {
      args: { query: 'k' }
    });
  });

  it('getFact passes id under {args.id}', async () => {
    await api.getFact('00000000-0000-0000-0000-000000000001');
    expect(invokeMock).toHaveBeenCalledWith('get_fact', {
      args: { id: '00000000-0000-0000-0000-000000000001' }
    });
  });

  it('getEvent passes id under {args.id}', async () => {
    await api.getEvent('00000000-0000-0000-0000-000000000002');
    expect(invokeMock).toHaveBeenCalledWith('get_event', {
      args: { id: '00000000-0000-0000-0000-000000000002' }
    });
  });

  it('getKnowledgeItem passes id under {args.id}', async () => {
    await api.getKnowledgeItem('00000000-0000-0000-0000-000000000003');
    expect(invokeMock).toHaveBeenCalledWith('get_knowledge_item', {
      args: { id: '00000000-0000-0000-0000-000000000003' }
    });
  });

  it('getHealth invokes without args envelope', async () => {
    await api.getHealth();
    expect(invokeMock).toHaveBeenCalledWith('get_health');
  });

  it('getConfig invokes without args envelope', async () => {
    await api.getConfig();
    expect(invokeMock).toHaveBeenCalledWith('get_config');
  });

  it('setConfig wraps fields under {args}', async () => {
    await api.setConfig({ klams_url: 'http://x', refresh_interval_seconds: 30 });
    expect(invokeMock).toHaveBeenCalledWith('set_config', {
      args: { klams_url: 'http://x', refresh_interval_seconds: 30 }
    });
  });
});
