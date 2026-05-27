<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/api';
  import type { PublicAuthor } from '$lib/types';
  import type { ListMemoriesResponse, MemoryItem } from '$lib/types/memories';
  import { hrefFor, summaryFor } from './row';

  let fromValue = $state('');
  let toValue = $state('');
  let kindFact = $state(true);
  let kindKnowledge = $state(true);
  let kindEvent = $state(true);
  let stateFilter = $state<'live' | 'deleted' | 'all'>('live');
  let selectedAuthors = $state<string[]>([]);
  let limit = $state(50);

  let authors = $state<PublicAuthor[]>([]);
  let page = $state<ListMemoriesResponse | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  function toLocalDateTime(d: Date): string {
    const y = d.getFullYear();
    const m = String(d.getMonth() + 1).padStart(2, '0');
    const day = String(d.getDate()).padStart(2, '0');
    const hh = String(d.getHours()).padStart(2, '0');
    const mm = String(d.getMinutes()).padStart(2, '0');
    return `${y}-${m}-${day}T${hh}:${mm}`;
  }

  function toIsoOrUndefined(local: string): string | undefined {
    if (!local) return undefined;
    const d = new Date(local);
    if (Number.isNaN(d.getTime())) return undefined;
    return d.toISOString();
  }

  function selectedKinds(): Array<'fact' | 'knowledge' | 'event'> {
    const kinds: Array<'fact' | 'knowledge' | 'event'> = [];
    if (kindFact) kinds.push('fact');
    if (kindKnowledge) kinds.push('knowledge');
    if (kindEvent) kinds.push('event');
    return kinds;
  }

  function formatError(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) {
      const o = e as { message?: unknown; status?: unknown };
      return `${String(o.message ?? '')}${o.status ? ` (${o.status})` : ''}`;
    }
    return String(e);
  }

  async function loadAuthors() {
    try {
      const a = await api.listAuthors({ limit: 200 });
      authors = a.authors;
    } catch {
      authors = [];
    }
  }

  async function load() {
    loading = true;
    error = null;
    try {
      page = await api.listMemories({
        since: toIsoOrUndefined(fromValue),
        until: toIsoOrUndefined(toValue),
        kinds: selectedKinds(),
        state: stateFilter,
        authors: selectedAuthors.length > 0 ? selectedAuthors : undefined,
        limit
      });
    } catch (e) {
      error = formatError(e);
    } finally {
      loading = false;
    }
  }

  async function loadNext() {
    if (!page?.next_cursor) return;
    loading = true;
    try {
      const next = await api.listMemories({
        since: toIsoOrUndefined(fromValue),
        until: toIsoOrUndefined(toValue),
        kinds: selectedKinds(),
        state: stateFilter,
        authors: selectedAuthors.length > 0 ? selectedAuthors : undefined,
        limit,
        cursor: page.next_cursor
      });
      page = {
        memories: [...(page?.memories ?? []), ...next.memories],
        next_cursor: next.next_cursor
      };
    } catch (e) {
      error = formatError(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    const now = new Date();
    const since = new Date(now.getTime() - 24 * 60 * 60 * 1000);
    fromValue = toLocalDateTime(since);
    toValue = toLocalDateTime(now);
    void loadAuthors();
    void load();
  });
</script>

<h1>Activity</h1>

<form onsubmit={(e) => { e.preventDefault(); void load(); }} class="filters">
  <label>From<input type="datetime-local" bind:value={fromValue} /></label>
  <label>To<input type="datetime-local" bind:value={toValue} /></label>
  <fieldset>
    <legend>Kinds</legend>
    <label><input type="checkbox" bind:checked={kindFact} /> fact</label>
    <label><input type="checkbox" bind:checked={kindKnowledge} /> knowledge</label>
    <label><input type="checkbox" bind:checked={kindEvent} /> event</label>
  </fieldset>
  <label>State
    <select bind:value={stateFilter}>
      <option value="live">live</option>
      <option value="deleted">soft-deleted</option>
      <option value="all">all</option>
    </select>
  </label>
  <label>Authors
    <select bind:value={selectedAuthors} multiple size="4">
      {#each authors as a (a.id)}
        <option value={a.id}>{a.agent_name}{a.model ? ` (${a.model})` : ''}</option>
      {/each}
    </select>
  </label>
  <label>Limit<input type="number" min="1" max="200" bind:value={limit} /></label>
  <button type="submit" disabled={loading}>{loading ? 'Loading…' : 'Apply'}</button>
</form>

{#if error}
  <p class="error">{error}</p>
{/if}

{#if page}
  <table>
    <thead>
      <tr>
        <th>State</th>
        <th>Kind</th>
        <th>Author</th>
        <th>Summary</th>
        <th>Updated</th>
        <th>Tags</th>
      </tr>
    </thead>
    <tbody>
      {#each page.memories as row (row.id)}
        <tr>
          <td>
            <span class={row.state === 'deleted' ? 'badge soft' : 'badge live'}>
              {row.state === 'deleted' ? 'soft-deleted' : 'live'}
            </span>
          </td>
          <td>{row.kind}</td>
          <td>{row.author.agent_name}</td>
          <td><a href={hrefFor(row)}>{summaryFor(row)}</a></td>
          <td>{new Date(row.updated_at).toLocaleString()}</td>
          <td>{row.tags?.join(', ') ?? ''}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if page.memories.length === 0}
    <p>No memories match these filters.</p>
  {/if}
  {#if page.next_cursor}
    <button type="button" onclick={() => void loadNext()} disabled={loading}>Load more</button>
  {/if}
{/if}

<style>
  .filters { display: flex; gap: 0.75rem; flex-wrap: wrap; align-items: end; margin-bottom: 1rem; }
  .filters label { display: flex; flex-direction: column; font-size: 0.85rem; }
  .filters fieldset { display: flex; gap: 0.5rem; align-items: center; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 0.4rem 0.6rem; border-bottom: 1px solid #ddd; text-align: left; vertical-align: top; }
  .badge { display: inline-block; padding: 0.1rem 0.5rem; border-radius: 0.25rem; font-size: 0.75rem; font-weight: 600; }
  .badge.live { background: #d4edda; color: #155724; }
  .badge.soft { background: #fff3cd; color: #856404; }
  .error { color: #b00; }
</style>
