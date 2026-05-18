<script lang="ts">
  import { api } from '$lib/api';
  import type { EventPage, KlamsEvent } from '$lib/types';

  let taskId = $state('');
  let category = $state('');
  let createdAfter = $state('');
  let createdBefore = $state('');
  let limit = $state(50);
  let page = $state<EventPage | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<KlamsEvent | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      page = await api.listEvents({
        task_id: taskId || undefined,
        category: category || undefined,
        created_after: createdAfter || undefined,
        created_before: createdBefore || undefined,
        limit
      });
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  function copyId(id: string) {
    void navigator.clipboard?.writeText(id);
  }
</script>

<h1>Events</h1>

<form onsubmit={(e) => { e.preventDefault(); void load(); }} class="filters">
  <label>Task id<input bind:value={taskId} placeholder="uuid" /></label>
  <label>Category<input bind:value={category} placeholder="agent.activity" /></label>
  <label>After<input type="datetime-local" bind:value={createdAfter} /></label>
  <label>Before<input type="datetime-local" bind:value={createdBefore} /></label>
  <label>Limit<input type="number" min="1" max="500" bind:value={limit} /></label>
  <button type="submit" disabled={loading}>Load</button>
</form>

{#if error}<p class="error">{error}</p>{/if}

{#if page}
  <table>
    <thead>
      <tr><th>Created at</th><th>Category</th><th>Task</th><th>Preview</th></tr>
    </thead>
    <tbody>
      {#each page.items as e (e.id)}
        <tr onclick={() => (selected = e)} class:selected={selected?.id === e.id}>
          <td>{e.created_at}</td>
          <td>{e.category}</td>
          <td>{e.task_id ?? '—'}</td>
          <td class="preview">{JSON.stringify(e.payload).slice(0, 80)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if page.items.length === 0}<p>No matching events.</p>{/if}
{/if}

{#if selected}
  <aside class="detail">
    <header>
      <h2>Event detail</h2>
      <button onclick={() => copyId(selected!.id)}>Copy id</button>
      <button onclick={() => (selected = null)}>Close</button>
    </header>
    <p><strong>id:</strong> {selected.id}</p>
    <p><strong>category:</strong> {selected.category} · <strong>source:</strong> {selected.source}</p>
    <pre>{JSON.stringify(selected.payload, null, 2)}</pre>
  </aside>
{/if}

<style>
  .filters { display: flex; gap: 0.75rem; flex-wrap: wrap; align-items: end; margin-bottom: 1rem; }
  .filters label { display: flex; flex-direction: column; font-size: 0.85rem; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 0.25rem 0.5rem; border-bottom: 1px solid #eee; text-align: left; font-size: 0.9rem; }
  tr.selected { background: #eef; }
  tr:hover { background: #f3f6fb; cursor: pointer; }
  .preview { font-family: ui-monospace, Menlo, monospace; }
  .detail { position: fixed; right: 1rem; top: 4rem; width: 420px; max-height: 80vh; overflow: auto; background: #fff; border: 1px solid #ddd; padding: 1rem; box-shadow: 0 4px 16px rgba(0,0,0,0.1); }
  .detail header { display: flex; gap: 0.5rem; align-items: center; }
  .detail header h2 { flex: 1; margin: 0; font-size: 1rem; }
  pre { background: #f7f7f7; padding: 0.5rem; overflow: auto; }
  .error { color: #c33; }
</style>
