<script lang="ts">
  import { api } from '$lib/api';
  import type { AuthorPage } from '$lib/types';

  let agentName = $state('');
  let since = $state('');
  let limit = $state(50);
  let page = $state<AuthorPage | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      page = await api.listAuthors({
        agent_name: agentName || undefined,
        since: since || undefined,
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
      const next = await api.listAuthors({
        agent_name: agentName || undefined,
        since: since || undefined,
        limit,
        cursor: page.next_cursor
      });
      page = {
        authors: [...(page?.authors ?? []), ...next.authors],
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
</script>

<h1>Authors</h1>

<form onsubmit={(e) => { e.preventDefault(); void load(); }} class="filters">
  <label>Agent name<input type="text" bind:value={agentName} placeholder="optional substring" /></label>
  <label>Since<input type="datetime-local" bind:value={since} /></label>
  <label>Limit<input type="number" min="1" max="200" bind:value={limit} /></label>
  <button type="submit" disabled={loading}>{loading ? 'Loading…' : 'Search'}</button>
</form>

{#if error}
  <p class="error">{error}</p>
{/if}

{#if page}
  <table>
    <thead>
      <tr>
        <th>Agent</th>
        <th>Model</th>
        <th>Repo</th>
        <th>Last seen</th>
        <th>Writes</th>
        <th>Events</th>
        <th>Soft deletes</th>
        <th>Restores</th>
      </tr>
    </thead>
    <tbody>
      {#each page.authors as a (a.id)}
        <tr>
          <td><a href={`/authors/${a.id}`}>{a.agent_name}</a></td>
          <td>{a.model ?? ''}</td>
          <td>{a.repo ?? ''}</td>
          <td>{new Date(a.last_seen_at).toLocaleString()}</td>
          <td>{a.counts.writes}</td>
          <td>{a.counts.events}</td>
          <td>{a.counts.soft_deletes}</td>
          <td>{a.counts.restores_received}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if page.authors.length === 0}
    <p>No authors match these filters.</p>
  {/if}
  {#if page.next_cursor}
    <button type="button" onclick={() => void loadNext()} disabled={loading}>Load more</button>
  {/if}
{/if}

<style>
  .filters { display: flex; gap: 0.75rem; flex-wrap: wrap; align-items: end; margin-bottom: 1rem; }
  .filters label { display: flex; flex-direction: column; font-size: 0.85rem; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 0.4rem 0.6rem; border-bottom: 1px solid #ddd; text-align: left; }
  .error { color: #b00; }
</style>
