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

  it('listDissents wraps filter args', async () => {
    await api.listDissents({ status: 'pending', limit: 10 });
    expect(invokeMock).toHaveBeenCalledWith('list_dissents', {
      args: { status: 'pending', limit: 10 }
    });
  });

  it('listDissents defaults args to empty object', async () => {
    await api.listDissents();
    expect(invokeMock).toHaveBeenCalledWith('list_dissents', { args: {} });
  });

  it('getDissent passes id under {args.id}', async () => {
    await api.getDissent('00000000-0000-0000-0000-000000000010');
    expect(invokeMock).toHaveBeenCalledWith('get_dissent', {
      args: { id: '00000000-0000-0000-0000-000000000010' }
    });
  });

  it('promoteDissent wraps caller_source + expected_version', async () => {
    await api.promoteDissent({
      dissent_id: '00000000-0000-0000-0000-000000000020',
      caller_source: 'User',
      expected_version: 3
    });
    expect(invokeMock).toHaveBeenCalledWith('promote_dissent', {
      args: {
        dissent_id: '00000000-0000-0000-0000-000000000020',
        caller_source: 'User',
        expected_version: 3
      }
    });
  });

  it('promoteDissent surfaces 403 trust_required as a rejection', async () => {
    invokeMock.mockRejectedValueOnce({ kind: 'server', status: 403, message: 'trust_required' });
    await expect(
      api.promoteDissent({
        dissent_id: '00000000-0000-0000-0000-000000000020',
        caller_source: 'AgentProposal',
        expected_version: 1
      })
    ).rejects.toMatchObject({ kind: 'server', status: 403 });
  });

  it('discardDissent wraps caller_source', async () => {
    await api.discardDissent({
      dissent_id: '00000000-0000-0000-0000-000000000030',
      caller_source: 'User'
    });
    expect(invokeMock).toHaveBeenCalledWith('discard_dissent', {
      args: { dissent_id: '00000000-0000-0000-0000-000000000030', caller_source: 'User' }
    });
  });

  it('upsertFact narrows on outcome=persisted', async () => {
    invokeMock.mockResolvedValueOnce({
      outcome: 'persisted',
      fact: { id: 'abc', version: 1 }
    });
    const out = await api.upsertFact({
      fact_type: 'UserFact',
      payload: { k: 'v' },
      source: 'User'
    });
    if (out.outcome === 'persisted') {
      expect(out.fact.id).toBe('abc');
    } else {
      throw new Error(`expected persisted, got ${out.outcome}`);
    }
  });

  it('upsertFact narrows on outcome=version_conflict', async () => {
    invokeMock.mockResolvedValueOnce({
      outcome: 'version_conflict',
      current_version: 7,
      fact_id: 'def'
    });
    const out = await api.upsertFact({
      fact_type: 'UserFact',
      payload: {},
      source: 'User',
      expected_version: 1
    });
    if (out.outcome === 'version_conflict') {
      expect(out.current_version).toBe(7);
    } else {
      throw new Error(`expected version_conflict, got ${out.outcome}`);
    }
  });

  it('editFact wraps id + expected_version', async () => {
    invokeMock.mockResolvedValueOnce({ outcome: 'persisted', fact: { id: 'x' } });
    await api.editFact({
      id: '00000000-0000-0000-0000-000000000040',
      fact_type: 'UserFact',
      payload: { k: 'v2' },
      expected_version: 2
    });
    expect(invokeMock).toHaveBeenCalledWith('edit_fact', {
      args: {
        id: '00000000-0000-0000-0000-000000000040',
        fact_type: 'UserFact',
        payload: { k: 'v2' },
        expected_version: 2
      }
    });
  });

  it('deleteFact passes id under {args.id}', async () => {
    await api.deleteFact('00000000-0000-0000-0000-000000000050');
    expect(invokeMock).toHaveBeenCalledWith('delete_fact', {
      args: { id: '00000000-0000-0000-0000-000000000050' }
    });
  });
});
