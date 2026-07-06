<script lang="ts">
  import { page as pageStore } from '$app/stores';
  import { api } from '$lib/api';
  import type { KlamsEvent } from '$lib/types';

  const eventId = $derived($pageStore.params.id);

  let event = $state<KlamsEvent | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function load() {
    if (!eventId) return;
    loading = true;
    error = null;
    try {
      event = await api.getEvent(eventId);
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
    if (eventId) void load();
  });
</script>

<a href="/events">&larr; Back to events</a>

{#if loading}
  <p>Loading…</p>
{:else if error}
  <p class="error">{error}</p>
{:else if event}
  <h1>Event <code class="id">{event.id}</code></h1>
  <dl class="meta">
    <dt>Category</dt><dd>{event.category}</dd>
    <dt>Source</dt><dd>{event.source}</dd>
    {#if event.task_id}
      <dt>Task</dt><dd><code>{event.task_id}</code></dd>
    {/if}
    <dt>Created</dt><dd>{new Date(event.created_at).toLocaleString()}</dd>
  </dl>
  <h2>Payload</h2>
  <pre>{payloadJson(event.payload)}</pre>
{/if}

<style>
  .id { font-size: 0.7em; }
  .meta { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 0.75rem; margin: 1rem 0; }
  .meta dt { font-weight: 600; }
  pre { background: #f6f6f6; padding: 0.75rem; border-radius: 0.25rem; overflow-x: auto; }
  .error { color: #b00; }
</style>
