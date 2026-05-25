<script lang="ts">
  import { page as pageStore } from '$app/stores';
  import { api } from '$lib/api';
  import type {
    AuthorMemoriesPage,
    AuthorMemoryRow,
    AuthorMemoryState,
    PublicAuthor
  } from '$lib/types';

  const authorId = $derived($pageStore.params.id);

  let author = $state<PublicAuthor | null>(null);
  let memPage = $state<AuthorMemoriesPage | null>(null);
  let stateFilter = $state<'live' | 'deleted' | 'all'>('live');
  let kindFact = $state(true);
  let kindKnowledge = $state(true);
  let kindEvent = $state(true);
  let limit = $state(50);
  let loading = $state(false);
  let error = $state<string | null>(null);

  function kindsParam(): string | undefined {
    const k: string[] = [];
    if (kindFact) k.push('fact');
    if (kindKnowledge) k.push('knowledge');
    if (kindEvent) k.push('event');
    if (k.length === 0 || k.length === 3) return undefined;
    return k.join(',');
  }

  async function loadAll() {
    if (!authorId) return;
    loading = true;
    error = null;
    try {
      const [a, m] = await Promise.all([
        api.getAuthor(authorId),
        api.listAuthorMemories({
          id: authorId,
          kinds: kindsParam(),
          state: stateFilter,
          limit
        })
      ]);
      author = a;
      memPage = m;
    } catch (e) {
      error = formatError(e);
    } finally {
      loading = false;
    }
  }

  async function loadNext() {
    if (!authorId || !memPage?.next_cursor) return;
    loading = true;
    try {
      const next = await api.listAuthorMemories({
        id: authorId,
        kinds: kindsParam(),
        state: stateFilter,
        limit,
        cursor: memPage.next_cursor
      });
      memPage = {
        memories: [...(memPage?.memories ?? []), ...next.memories],
        next_cursor: next.next_cursor
      };
    } catch (e) {
      error = formatError(e);
    } finally {
      loading = false;
    }
  }

  function formatError(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) {
      const o = e as { message?: unknown; status?: unknown };
      return `${String(o.message ?? '')}${o.status ? ` (${o.status})` : ''}`;
    }
    return String(e);
  }

  function badgeFor(row: AuthorMemoryRow): { label: string; cls: string } {
    const s = row.state as AuthorMemoryState | 'hard-deleted';
    if (s === 'live') return { label: 'live', cls: 'badge live' };
    if (s === 'deleted') return { label: 'soft-deleted', cls: 'badge soft' };
    return { label: 'hard-deleted', cls: 'badge hard' };
  }

  function hrefFor(row: AuthorMemoryRow): string {
    switch (row.kind) {
      case 'fact':
        return `/facts/${row.id}`;
      case 'knowledge':
        return `/knowledge/${row.id}`;
      case 'event':
        return `/events/${row.id}`;
    }
  }

  function summaryFor(row: AuthorMemoryRow): string {
    switch (row.kind) {
      case 'fact':
        return `${row.type}: ${JSON.stringify(row.payload).slice(0, 120)}`;
      case 'knowledge':
        return row.text.slice(0, 160);
      case 'event':
        return `${row.category}: ${JSON.stringify(row.payload).slice(0, 120)}`;
    }
  }

  $effect(() => {
    if (authorId) void loadAll();
  });
</script>

<a href="/authors">&larr; Back to authors</a>

{#if error}
  <p class="error">{error}</p>
{/if}

{#if author}
  <h1>{author.agent_name}</h1>
  <dl class="meta">
    <dt>ID</dt><dd><code>{author.id}</code></dd>
    {#if author.model}<dt>Model</dt><dd>{author.model}</dd>{/if}
    {#if author.session_title}<dt>Session</dt><dd>{author.session_title}</dd>{/if}
    {#if author.repo}<dt>Repo</dt><dd>{author.repo}</dd>{/if}
    {#if author.client_app}<dt>Client</dt><dd>{author.client_app}{author.client_version ? ` ${author.client_version}` : ''}</dd>{/if}
    <dt>First seen</dt><dd>{new Date(author.created_at).toLocaleString()}</dd>
    <dt>Last seen</dt><dd>{new Date(author.last_seen_at).toLocaleString()}</dd>
    <dt>Counts</dt>
    <dd>
      writes={author.counts.writes}
      &middot; events={author.counts.events}
      &middot; soft-deletes={author.counts.soft_deletes}
      &middot; restores={author.counts.restores_received}
    </dd>
  </dl>
{/if}

<h2>Memories</h2>

<form onsubmit={(e) => { e.preventDefault(); void loadAll(); }} class="filters">
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
  <label>Limit<input type="number" min="1" max="200" bind:value={limit} /></label>
  <button type="submit" disabled={loading}>{loading ? 'Loading…' : 'Apply'}</button>
</form>

{#if memPage}
  <table>
    <thead>
      <tr>
        <th>State</th>
        <th>Kind</th>
        <th>Summary</th>
        <th>Updated</th>
        <th>Tags</th>
      </tr>
    </thead>
    <tbody>
      {#each memPage.memories as row (row.id)}
        {@const b = badgeFor(row)}
        <tr>
          <td><span class={b.cls}>{b.label}</span></td>
          <td>{row.kind}</td>
          <td><a href={hrefFor(row)}>{summaryFor(row)}</a></td>
          <td>{new Date(row.updated_at).toLocaleString()}</td>
          <td>{row.tags?.join(', ') ?? ''}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if memPage.memories.length === 0}
    <p>No memories match these filters.</p>
  {/if}
  {#if memPage.next_cursor}
    <button type="button" onclick={() => void loadNext()} disabled={loading}>Load more</button>
  {/if}
{/if}

<style>
  .meta { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 0.75rem; margin: 1rem 0; }
  .meta dt { font-weight: 600; }
  .filters { display: flex; gap: 0.75rem; flex-wrap: wrap; align-items: end; margin-bottom: 1rem; }
  .filters fieldset { display: flex; gap: 0.5rem; align-items: center; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 0.4rem 0.6rem; border-bottom: 1px solid #ddd; text-align: left; vertical-align: top; }
  .badge { display: inline-block; padding: 0.1rem 0.5rem; border-radius: 0.25rem; font-size: 0.75rem; font-weight: 600; }
  .badge.live { background: #d4edda; color: #155724; }
  .badge.soft { background: #fff3cd; color: #856404; }
  .badge.hard { background: #f8d7da; color: #721c24; }
  .error { color: #b00; }
</style>
