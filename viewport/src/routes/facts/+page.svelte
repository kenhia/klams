<script lang="ts">
  import { writable } from 'svelte/store';
  import { api } from '$lib/api';
  import type { Fact, FactPage, ProvenanceBundle, Source, UpsertResult } from '$lib/types';
  import ProvenancePanel from '$lib/ProvenancePanel.svelte';
  import { withOptimistic } from '$lib/optimistic';
  import { mutationCounter } from '$lib/stores';

  let factType = $state('');
  let source = $state('');
  let createdAfter = $state('');
  let createdBefore = $state('');
  let limit = $state(50);
  const factsStore = writable<FactPage | null>(null);
  let page = $state<FactPage | null>(null);
  factsStore.subscribe((v) => (page = v));
  let loading = $state(false);
  let error = $state<string | null>(null);
  let toast = $state<string | null>(null);
  let selected = $state<Fact | null>(null);
  let editing = $state<Fact | null>(null);
  let editText = $state('');
  let confirmingDelete = $state<Fact | null>(null);

  async function load() {
    loading = true;
    error = null;
    try {
      const next = await api.listFacts({
        fact_type: factType || undefined,
        source: source || undefined,
        created_after: createdAfter || undefined,
        created_before: createdBefore || undefined,
        limit
      });
      factsStore.set(next);
    } catch (e) {
      error = formatError(e);
    } finally {
      loading = false;
    }
  }

  function formatError(e: unknown): string {
    if (e && typeof e === 'object' && 'message' in e) {
      const o = e as { message?: unknown; details?: unknown; status?: unknown };
      const bits = [String(o.message ?? '')];
      if (o.status) bits.push(`(${o.status})`);
      if (o.details) bits.push(JSON.stringify(o.details));
      return bits.filter(Boolean).join(' ');
    }
    return String(e);
  }

  function copyId(id: string) {
    void navigator.clipboard?.writeText(id);
  }

  function startEdit(f: Fact) {
    editing = f;
    editText = JSON.stringify(f.payload, null, 2);
  }

  async function saveEdit() {
    if (!editing) return;
    const id = editing.id;
    const factTypeValue = editing.fact_type;
    const expected = editing.version;
    let parsed: unknown;
    try {
      parsed = JSON.parse(editText);
    } catch (e) {
      toast = `Invalid JSON: ${String(e)}`;
      return;
    }
    const target = editing;
    try {
      const out: UpsertResult = await withOptimistic(
        factsStore,
        (cur) => {
          if (!cur) return cur;
          return {
            ...cur,
            items: cur.items.map((f) =>
              f.id === id ? { ...f, payload: parsed, version: f.version + 1 } : f
            )
          };
        },
        () =>
          api.editFact({
            id,
            fact_type: factTypeValue,
            payload: parsed,
            expected_version: expected
          })
      );
      mutationCounter.update((n) => n + 1);
      editing = null;
      if (out.outcome === 'version_conflict') {
        toast = `Version conflict: server is at v${out.current_version}. Reload and retry.`;
        await load();
      } else if (out.outcome === 'dissented') {
        toast = `Edit was filed as a dissent (id ${out.dissent_id}). Visit /dissents to triage.`;
        await load();
      } else if (selected?.id === target.id) {
        selected = out.fact;
      }
    } catch (e) {
      toast = `Edit failed: ${formatError(e)}`;
    }
  }

  async function confirmDelete() {
    if (!confirmingDelete) return;
    const target = confirmingDelete;
    confirmingDelete = null;
    try {
      await withOptimistic(
        factsStore,
        (cur) =>
          cur ? { ...cur, items: cur.items.filter((f) => f.id !== target.id) } : cur,
        () => api.deleteFact(target.id)
      );
      mutationCounter.update((n) => n + 1);
      if (selected?.id === target.id) selected = null;
    } catch (e) {
      toast = `Delete failed: ${formatError(e)}`;
    }
  }

  function provenanceOf(f: Fact): ProvenanceBundle {
    return {
      source: f.source as Source,
      confidence: f.confidence,
      decay_weight: f.decay_weight,
      use_count: f.use_count,
      last_used_at: f.last_used_at,
      created_at: f.created_at,
      updated_at: f.updated_at,
      version: f.version
    };
  }
</script>

<h1>Facts</h1>

<form onsubmit={(e) => { e.preventDefault(); void load(); }} class="filters">
  <label>Type
    <select bind:value={factType}>
      <option value="">any</option>
      <option value="UserFact">UserFact</option>
      <option value="TaskFact">TaskFact</option>
      <option value="EnvFact">EnvFact</option>
    </select>
  </label>
  <label>Source
    <select bind:value={source}>
      <option value="">any</option>
      <option value="User">User</option>
      <option value="Controller">Controller</option>
      <option value="Task">Task</option>
      <option value="AgentProposal">AgentProposal</option>
    </select>
  </label>
  <label>Created after<input type="datetime-local" bind:value={createdAfter} /></label>
  <label>Created before<input type="datetime-local" bind:value={createdBefore} /></label>
  <label>Limit<input type="number" min="1" max="500" bind:value={limit} /></label>
  <button type="submit" disabled={loading}>Load</button>
</form>

{#if error}<p class="error">{error}</p>{/if}
{#if toast}<p class="toast" onclick={() => (toast = null)} role="presentation">{toast}</p>{/if}

{#if page}
  <table>
    <thead>
      <tr>
        <th>Payload preview</th>
        <th>Confidence</th>
        <th>Decay</th>
        <th>Last used</th>
        <th>Uses</th>
        <th>Dissents</th>
        <th></th>
      </tr>
    </thead>
    <tbody>
      {#each page.items as f (f.id)}
        <tr onclick={() => (selected = f)} class:selected={selected?.id === f.id}>
          <td class="preview">{JSON.stringify(f.payload).slice(0, 100)}</td>
          <td>{f.confidence.toFixed(2)}</td>
          <td>{f.decay_weight.toFixed(2)}</td>
          <td>{f.last_used_at ?? '—'}</td>
          <td>{f.use_count}</td>
          <td>
            {#if f.dissent_count > 0}
              <a href={`/dissents?fact_id=${f.id}`} onclick={(e) => e.stopPropagation()}>
                {f.dissent_count}
              </a>
            {:else}
              0
            {/if}
          </td>
          <td>
            <button onclick={(e) => { e.stopPropagation(); startEdit(f); }}>Edit</button>
            <button onclick={(e) => { e.stopPropagation(); confirmingDelete = f; }}>Delete</button>
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if page.items.length === 0}<p>No matching facts.</p>{/if}
{/if}

{#if selected}
  <aside class="detail">
    <header>
      <h2>Fact detail</h2>
      <button onclick={() => copyId(selected!.id)}>Copy id</button>
      <button onclick={() => (selected = null)}>Close</button>
    </header>
    <p><strong>id:</strong> {selected.id}</p>
    <p><strong>type:</strong> {selected.fact_type}</p>
    <ProvenancePanel
      provenance={provenanceOf(selected)}
      dissentCount={selected.dissent_count}
      factId={selected.id}
    />
    <pre>{JSON.stringify(selected.payload, null, 2)}</pre>
  </aside>
{/if}

{#if editing}
  <div class="modal-backdrop" onclick={() => (editing = null)} role="presentation"></div>
  <div class="modal" role="dialog" aria-modal="true">
    <h2>Edit fact (User-sourced)</h2>
    <p class="muted">id: {editing.id} · expected_version: {editing.version}</p>
    <textarea bind:value={editText} rows="14"></textarea>
    <div class="actions">
      <button onclick={() => (editing = null)}>Cancel</button>
      <button onclick={() => void saveEdit()}>Save</button>
    </div>
  </div>
{/if}

{#if confirmingDelete}
  <div class="modal-backdrop" onclick={() => (confirmingDelete = null)} role="presentation"></div>
  <div class="modal" role="dialog" aria-modal="true">
    <h2>Delete fact?</h2>
    <p>id: {confirmingDelete.id}</p>
    <pre>{JSON.stringify(confirmingDelete.payload, null, 2)}</pre>
    <p class="muted">This cannot be undone. The viewport will refresh on failure.</p>
    <div class="actions">
      <button onclick={() => (confirmingDelete = null)}>Cancel</button>
      <button onclick={() => void confirmDelete()}>Delete</button>
    </div>
  </div>
{/if}

<style>
  .filters { display: flex; gap: 0.75rem; flex-wrap: wrap; align-items: end; margin-bottom: 1rem; }
  .filters label { display: flex; flex-direction: column; font-size: 0.85rem; }
  table { width: 100%; border-collapse: collapse; }
  th, td { padding: 0.25rem 0.5rem; border-bottom: 1px solid #eee; text-align: left; font-size: 0.9rem; }
  tr.selected { background: #eef; }
  tr:hover { background: #f3f6fb; cursor: pointer; }
  .preview { font-family: ui-monospace, Menlo, monospace; }
  .detail { position: fixed; right: 1rem; top: 4rem; width: 460px; max-height: 80vh; overflow: auto; background: #fff; border: 1px solid #ddd; padding: 1rem; box-shadow: 0 4px 16px rgba(0,0,0,0.1); }
  .detail header { display: flex; gap: 0.5rem; align-items: center; }
  .detail header h2 { flex: 1; margin: 0; font-size: 1rem; }
  pre { background: #f7f7f7; padding: 0.5rem; overflow: auto; }
  .error { color: #c33; }
  .toast { background: #fdd; border: 1px solid #c66; padding: 0.5rem; cursor: pointer; }
  .modal-backdrop { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.3); }
  .modal { position: fixed; top: 10%; left: 50%; transform: translateX(-50%); background: #fff; padding: 1.5rem; border-radius: 8px; min-width: 480px; box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2); }
  .modal textarea { width: 100%; font-family: ui-monospace, Menlo, monospace; }
  .actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.5rem; }
  .muted { color: #777; font-size: 0.85rem; }
</style>
