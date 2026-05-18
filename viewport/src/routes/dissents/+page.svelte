<script lang="ts">
  import { onMount } from 'svelte';
  import { writable } from 'svelte/store';
  import { page as pageStore } from '$app/stores';
  import { api } from '$lib/api';
  import type { Dissent, DissentPage, DissentStatus, Fact, Source } from '$lib/types';
  import { withOptimistic } from '$lib/optimistic';
  import { mutationCounter, refreshPendingDissents } from '$lib/stores';

  let statusFilter = $state<DissentStatus>('pending');
  let sourceFilter = $state<Source | ''>('');
  let factIdFilter = $state('');
  let createdAfter = $state('');
  let callerSource = $state<Source>('User');

  const dissentsStore = writable<DissentPage | null>(null);
  let pageValue = $state<DissentPage | null>(null);
  dissentsStore.subscribe((v) => (pageValue = v));
  let loading = $state(false);
  let error = $state<string | null>(null);
  let toast = $state<string | null>(null);

  // Cache of canonical fact payloads (for diff display) keyed by fact_id.
  let factCache = $state<Record<string, Fact | null>>({});

  onMount(() => {
    factIdFilter = $pageStore.url.searchParams.get('fact_id') ?? '';
    void load();
  });

  function formatError(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) {
      const o = e as { message?: unknown; status?: unknown; details?: unknown };
      const bits = [String(o.message ?? '')];
      if (o.status) bits.push(`(${o.status})`);
      if (o.details) bits.push(JSON.stringify(o.details));
      return bits.filter(Boolean).join(' ');
    }
    return String(e);
  }

  async function load() {
    loading = true;
    error = null;
    try {
      const next = await api.listDissents({
        status: statusFilter,
        source: sourceFilter || undefined,
        fact_id: factIdFilter || undefined,
        created_after: createdAfter || undefined,
        limit: 200
      });
      dissentsStore.set(next);
      await Promise.all(
        next.items.map((d) => {
          if (factCache[d.fact_id] !== undefined) return Promise.resolve();
          return api
            .getFact(d.fact_id)
            .then((f) => (factCache = { ...factCache, [d.fact_id]: f }))
            .catch(() => (factCache = { ...factCache, [d.fact_id]: null }));
        })
      );
    } catch (e) {
      error = formatError(e);
    } finally {
      loading = false;
    }
  }

  async function refreshRow(id: string) {
    try {
      const d = await api.getDissent(id);
      dissentsStore.update((cur) => {
        if (!cur) return cur;
        return { ...cur, items: cur.items.map((row) => (row.id === id ? d : row)) };
      });
    } catch {
      /* row may be gone; do nothing */
    }
  }

  async function promote(d: Dissent) {
    const canonical = factCache[d.fact_id];
    if (!canonical) {
      toast = 'Cannot promote: canonical fact not yet loaded.';
      return;
    }
    try {
      await withOptimistic(
        dissentsStore,
        (cur) =>
          cur ? { ...cur, items: cur.items.filter((row) => row.id !== d.id) } : cur,
        () =>
          api.promoteDissent({
            dissent_id: d.id,
            caller_source: callerSource,
            expected_version: canonical.version
          })
      );
      mutationCounter.update((n) => n + 1);
      void refreshPendingDissents();
      // Pull fresh canonical for any future diffs.
      try {
        factCache = {
          ...factCache,
          [d.fact_id]: await api.getFact(d.fact_id)
        };
      } catch {
        /* deleted? leave as-is */
      }
    } catch (e) {
      const o = e as { status?: number };
      if (o && o.status === 410) {
        toast = 'Dissent is gone (orphaned or already resolved). Refreshing row.';
        await refreshRow(d.id);
      } else if (o && o.status === 409) {
        toast = 'Version conflict — canonical changed. Reloading list.';
        await load();
      } else if (o && o.status === 403) {
        toast = `Promotion refused: ${formatError(e)}`;
        await load();
      } else {
        toast = `Promote failed: ${formatError(e)}`;
        await load();
      }
    }
  }

  async function discard(d: Dissent) {
    try {
      await withOptimistic(
        dissentsStore,
        (cur) =>
          cur ? { ...cur, items: cur.items.filter((row) => row.id !== d.id) } : cur,
        () => api.discardDissent({ dissent_id: d.id, caller_source: callerSource })
      );
      mutationCounter.update((n) => n + 1);
      void refreshPendingDissents();
    } catch (e) {
      const o = e as { status?: number };
      if (o && o.status === 410) {
        toast = 'Dissent already resolved; refreshing row.';
        await refreshRow(d.id);
      } else {
        toast = `Discard failed: ${formatError(e)}`;
        await load();
      }
    }
  }
</script>

<h1>Dissents</h1>

<form onsubmit={(e) => { e.preventDefault(); void load(); }} class="filters">
  <label>Status
    <select bind:value={statusFilter}>
      <option value="pending">pending</option>
      <option value="promoted">promoted</option>
      <option value="discarded">discarded</option>
      <option value="orphaned">orphaned</option>
    </select>
  </label>
  <label>Source
    <select bind:value={sourceFilter}>
      <option value="">any</option>
      <option value="User">User</option>
      <option value="Controller">Controller</option>
      <option value="Task">Task</option>
      <option value="AgentProposal">AgentProposal</option>
    </select>
  </label>
  <label>Fact id
    <input bind:value={factIdFilter} placeholder="uuid" />
  </label>
  <label>Created after<input type="datetime-local" bind:value={createdAfter} /></label>
  <label>Caller source
    <select bind:value={callerSource}>
      <option value="User">User</option>
      <option value="Controller">Controller</option>
    </select>
  </label>
  <button type="submit" disabled={loading}>Reload</button>
</form>

{#if error}<p class="error">{error}</p>{/if}
{#if toast}<p class="toast" onclick={() => (toast = null)} role="presentation">{toast}</p>{/if}

{#if pageValue}
  {#if pageValue.items.length === 0}
    <p>No matching dissents.</p>
  {/if}
  {#each pageValue.items as d (d.id)}
    <article class="dissent">
      <header>
        <span class="src">{d.source}</span>
        <span class="when">submitted {d.submitted_at} · seen ×{d.submission_count}</span>
        <span class="fid">fact <code>{d.fact_id}</code></span>
      </header>
      <div class="diff">
        <section>
          <h3>Canonical</h3>
          {#if factCache[d.fact_id] === undefined}
            <p class="muted">loading…</p>
          {:else if factCache[d.fact_id] === null}
            <p class="muted">canonical fact unavailable (404)</p>
          {:else}
            <pre>{JSON.stringify(factCache[d.fact_id]?.payload, null, 2)}</pre>
            <p class="muted">v{factCache[d.fact_id]?.version}</p>
          {/if}
        </section>
        <section>
          <h3>Proposed</h3>
          <pre>{JSON.stringify(d.proposed_payload, null, 2)}</pre>
        </section>
      </div>
      {#if d.status === 'pending'}
        <footer class="actions">
          <button onclick={() => void promote(d)}>Promote</button>
          <button onclick={() => void discard(d)}>Discard</button>
        </footer>
      {:else}
        <footer class="resolved">
          {d.status} {d.resolved_at ? `at ${d.resolved_at}` : ''} by {d.resolved_by_source ?? '—'}
        </footer>
      {/if}
    </article>
  {/each}
{/if}

<style>
  .filters { display: flex; gap: 0.75rem; flex-wrap: wrap; align-items: end; margin-bottom: 1rem; }
  .filters label { display: flex; flex-direction: column; font-size: 0.85rem; }
  .error { color: #c33; }
  .toast { background: #fdd; border: 1px solid #c66; padding: 0.5rem; cursor: pointer; }
  .dissent { border: 1px solid #ddd; padding: 0.75rem; margin-bottom: 0.75rem; border-radius: 6px; background: #fff; }
  .dissent header { display: flex; gap: 0.75rem; align-items: center; font-size: 0.85rem; color: #555; margin-bottom: 0.5rem; }
  .src { font-weight: 600; color: #246; }
  .fid code { font-family: ui-monospace, Menlo, monospace; }
  .diff { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem; }
  .diff h3 { margin: 0 0 0.25rem; font-size: 0.9rem; }
  pre { background: #f7f7f7; padding: 0.5rem; overflow: auto; margin: 0; font-size: 0.85rem; }
  .actions { margin-top: 0.5rem; display: flex; gap: 0.5rem; justify-content: flex-end; }
  .resolved { margin-top: 0.5rem; color: #777; font-size: 0.85rem; text-align: right; }
  .muted { color: #777; font-size: 0.85rem; margin: 0; }
</style>
