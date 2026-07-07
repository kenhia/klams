<script lang="ts">
  import { page as pageStore } from '$app/stores';
  import { api } from '$lib/api';
  import type { KnowledgeItem } from '$lib/types';

  const itemId = $derived($pageStore.params.id);

  let item = $state<KnowledgeItem | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function load() {
    if (!itemId) return;
    loading = true;
    error = null;
    try {
      item = await api.getKnowledgeItem(itemId);
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

  $effect(() => {
    if (itemId) void load();
  });
</script>

<a href="/knowledge">&larr; Back to knowledge</a>

{#if loading}
  <p>Loading…</p>
{:else if error}
  <p class="error">{error}</p>
{:else if item}
  <h1>Knowledge <code class="id">{item.id}</code></h1>
  <dl class="meta">
    <dt>Source</dt><dd>{item.source}</dd>
    {#if item.repo}<dt>Repo</dt><dd>{item.repo}</dd>{/if}
    {#if item.file}<dt>File</dt><dd><code>{item.file}</code></dd>{/if}
    {#if item.machine}<dt>Machine</dt><dd>{item.machine}</dd>{/if}
    {#if item.tags.length > 0}<dt>Tags</dt><dd>{item.tags.join(', ')}</dd>{/if}
    <dt>Confidence</dt><dd>{item.confidence}</dd>
    <dt>Decay weight</dt><dd>{item.decay_weight}</dd>
    <dt>Use count</dt><dd>{item.use_count}</dd>
    <dt>Created</dt><dd>{new Date(item.created_at).toLocaleString()}</dd>
    <dt>Updated</dt><dd>{new Date(item.updated_at).toLocaleString()}</dd>
    {#if item.last_used_at}
      <dt>Last used</dt><dd>{new Date(item.last_used_at).toLocaleString()}</dd>
    {/if}
  </dl>
  <h2>Text</h2>
  <pre>{item.text}</pre>
{/if}

<style>
  .id { font-size: 0.7em; }
  .meta { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 0.75rem; margin: 1rem 0; }
  .meta dt { font-weight: 600; }
  pre { background: #f6f6f6; padding: 0.75rem; border-radius: 0.25rem; overflow-x: auto; white-space: pre-wrap; }
  .error { color: #b00; }
</style>
