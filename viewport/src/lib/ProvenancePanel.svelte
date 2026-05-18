<!-- ProvenancePanel: pure presentational. Renders the 8 provenance
     fields (FR-019) for a Fact or KnowledgeItem, plus a footer link
     to the dissents triage page when `dissentCount > 0`. -->
<script lang="ts">
  import type { ProvenanceBundle } from './types';

  let {
    provenance,
    dissentCount = 0,
    factId
  }: {
    provenance: ProvenanceBundle;
    dissentCount?: number;
    factId?: string;
  } = $props();
</script>

<dl class="provenance">
  <dt>Source</dt>
  <dd>{provenance.source}</dd>

  {#if provenance.version != null}
    <dt>Version</dt>
    <dd>{provenance.version}</dd>
  {/if}

  <dt>Confidence</dt>
  <dd>{provenance.confidence.toFixed(2)}</dd>

  <dt>Decay weight</dt>
  <dd>{provenance.decay_weight.toFixed(2)}</dd>

  <dt>Use count</dt>
  <dd>{provenance.use_count}</dd>

  <dt>Last used</dt>
  <dd>{provenance.last_used_at ?? '—'}</dd>

  <dt>Created</dt>
  <dd>{provenance.created_at}</dd>

  <dt>Updated</dt>
  <dd>{provenance.updated_at}</dd>
</dl>

{#if dissentCount > 0}
  <p class="dissents">
    {#if factId}
      <a href={`/dissents?fact_id=${factId}`}>Pending dissents: {dissentCount}</a>
    {:else}
      <a href="/dissents">Pending dissents: {dissentCount}</a>
    {/if}
  </p>
{/if}

<style>
  .provenance {
    display: grid;
    grid-template-columns: max-content 1fr;
    column-gap: 0.75rem;
    row-gap: 0.2rem;
    font-size: 0.85rem;
    margin: 0.5rem 0;
  }
  .provenance dt {
    color: #555;
  }
  .provenance dd {
    margin: 0;
    font-family: ui-monospace, Menlo, monospace;
  }
  .dissents {
    margin: 0.25rem 0 0.5rem;
    font-size: 0.85rem;
  }
  .dissents a {
    color: #b35;
  }
</style>
