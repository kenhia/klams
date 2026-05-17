<script lang="ts">
  import { api } from '$lib/api';
  import type { KnowledgeItem, SearchResults } from '$lib/types';

  let query = $state('');
  let topK = $state(10);
  let results = $state<SearchResults | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);
  let selected = $state<KnowledgeItem | null>(null);
  let selectedError = $state<string | null>(null);

  async function search() {
    loading = true;
    error = null;
    try {
      results = await api.searchKnowledge({ query, top_k: topK });
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function open(id: string) {
    selected = null;
    selectedError = null;
    try {
      selected = await api.getKnowledgeItem(id);
    } catch (e) {
      selectedError = String(e);
    }
  }
</script>

<h1>Knowledge</h1>

<form onsubmit={(e) => { e.preventDefault(); void search(); }} class="filters">
  <label>Query<input bind:value={query} placeholder="search text" /></label>
  <label>Top k<input type="number" min="1" max="100" bind:value={topK} /></label>
  <button type="submit" disabled={loading || !query.trim()}>Search</button>
</form>

{#if error}<p class="error">{error}</p>{/if}

{#if results}
  {#if results.degraded}<p class="warn">Degraded: knowledge backend unavailable for this query.</p>{/if}
  <p>{results.total} result(s)</p>
  <table>
    <thead><tr><th>Score</th><th>Preview</th></tr></thead>
    <tbody>
      {#each results.results as r (r.id)}
        <tr onclick={() => void open(r.id)} class:selected={selected?.id === r.id}>
          <td>{r.score.toFixed(3)}</td>
          <td class="preview">{r.preview}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/if}

{#if selectedError}<p class="error">{selectedError}</p>{/if}

{#if selected}
  <aside class="detail">
    <header>
      <h2>Knowledge item</h2>
      <button onclick={() => (selected = null)}>Close</button>
    </header>
    <p><strong>id:</strong> {selected.id}</p>
    <p><strong>tags:</strong> {selected.tags.join(', ') || '—'}</p>
    <p><strong>repo:</strong> {selected.repo ?? '—'} · <strong>file:</strong> {selected.file ?? '—'} · <strong>machine:</strong> {selected.machine ?? '—'}</p>
    <p><strong>confidence:</strong> {selected.confidence} · <strong>decay:</strong> {selected.decay_weight} · <strong>uses:</strong> {selected.use_count}</p>
    <pre>{selected.text}</pre>
  </aside>
{/if}

<style>
  .filters { display: flex; gap: 0.75rem; align-items: end; margin-bottom: 1rem; }
  .filters label { display: flex; flex-direction: column; font-size: 0.85rem; flex: 1; }
  .filters input { padding: 0.4rem; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 0.25rem 0.5rem; border-bottom: 1px solid #eee; text-align: left; font-size: 0.9rem; }
  tr.selected { background: #eef; }
  tr:hover { background: #f3f6fb; cursor: pointer; }
  .preview { font-family: ui-monospace, Menlo, monospace; }
  .detail { position: fixed; right: 1rem; top: 4rem; width: 480px; max-height: 80vh; overflow: auto; background: #fff; border: 1px solid #ddd; padding: 1rem; box-shadow: 0 4px 16px rgba(0,0,0,0.1); }
  .detail header { display: flex; gap: 0.5rem; align-items: center; }
  .detail header h2 { flex: 1; margin: 0; font-size: 1rem; }
  pre { background: #f7f7f7; padding: 0.5rem; overflow: auto; white-space: pre-wrap; }
  .error { color: #c33; }
  .warn { color: #c80; }
</style>
