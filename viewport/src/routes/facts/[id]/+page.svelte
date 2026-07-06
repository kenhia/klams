<script lang="ts">
  import { page as pageStore } from '$app/stores';
  import { api } from '$lib/api';
  import type { Fact } from '$lib/types';

  const factId = $derived($pageStore.params.id);

  let fact = $state<Fact | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function load() {
    if (!factId) return;
    loading = true;
    error = null;
    try {
      fact = await api.getFact(factId);
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

  function payloadJson(p: unknown): string {
    try {
      return JSON.stringify(p, null, 2);
    } catch {
      return String(p);
    }
  }

  $effect(() => {
    if (factId) void load();
  });
</script>

<a href="/facts">&larr; Back to facts</a>

{#if loading}
  <p>Loading…</p>
{:else if error}
  <p class="error">{error}</p>
{:else if fact}
  <h1>Fact <code class="id">{fact.id}</code></h1>
  <dl class="meta">
    <dt>Type</dt><dd>{fact.fact_type}</dd>
    <dt>Source</dt><dd>{fact.source}</dd>
    <dt>Version</dt><dd>{fact.version}</dd>
    <dt>Confidence</dt><dd>{fact.confidence}</dd>
    <dt>Decay weight</dt><dd>{fact.decay_weight}</dd>
    <dt>Use count</dt><dd>{fact.use_count}</dd>
    <dt>Pending dissents</dt>
    <dd>
      {fact.dissent_count}
      {#if fact.dissent_count > 0}<a href={`/dissents?fact_id=${fact.id}`}>view</a>{/if}
    </dd>
    <dt>Created</dt><dd>{new Date(fact.created_at).toLocaleString()}</dd>
    <dt>Updated</dt><dd>{new Date(fact.updated_at).toLocaleString()}</dd>
    {#if fact.last_used_at}
      <dt>Last used</dt><dd>{new Date(fact.last_used_at).toLocaleString()}</dd>
    {/if}
  </dl>
  <h2>Payload</h2>
  <pre>{payloadJson(fact.payload)}</pre>
{/if}

<style>
  .id { font-size: 0.7em; }
  .meta { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 0.75rem; margin: 1rem 0; }
  .meta dt { font-weight: 600; }
  pre { background: #f6f6f6; padding: 0.75rem; border-radius: 0.25rem; overflow-x: auto; }
  .error { color: #b00; }
</style>
