<script lang="ts">
  import type { MemoryItem } from '$lib/types/memories';

  interface Props {
    item: MemoryItem;
  }

  let { item }: Props = $props();

  function payloadJson(p: unknown): string {
    try {
      return JSON.stringify(p, null, 2);
    } catch {
      return String(p);
    }
  }
</script>

<div class="details">
  <dl>
    <dt>ID</dt>
    <dd><code>{item.id}</code></dd>
    <dt>Kind</dt>
    <dd>{item.kind}</dd>
    <dt>Author</dt>
    <dd>
      {item.author.agent_name}
      {#if item.author.model}<small>({item.author.model})</small>{/if}
    </dd>
    <dt>Created</dt>
    <dd>{new Date(item.created_at).toLocaleString()}</dd>
    <dt>Updated</dt>
    <dd>{new Date(item.updated_at).toLocaleString()}</dd>
    <dt>State</dt>
    <dd>{item.state}</dd>
    {#if item.state === 'deleted' && item.deleted_at}
      <dt>Deleted</dt>
      <dd>
        {new Date(item.deleted_at).toLocaleString()}
        {#if item.deleted_by}by {item.deleted_by.agent_name}{/if}
      </dd>
    {/if}
    {#if item.tags && item.tags.length > 0}
      <dt>Tags</dt>
      <dd>{item.tags.join(', ')}</dd>
    {/if}
  </dl>

  {#if item.kind === 'fact'}
    <h4>Fact ({item.type})</h4>
    <pre>{payloadJson(item.payload)}</pre>
  {:else if item.kind === 'knowledge'}
    <h4>Knowledge</h4>
    {#if item.source_path}
      <p class="meta">Source: <code>{item.source_path}</code>{#if item.repo} · Repo: <code>{item.repo}</code>{/if}</p>
    {/if}
    <pre class="text">{item.text}</pre>
  {:else if item.kind === 'event'}
    <h4>Event ({item.category})</h4>
    {#if item.task_id}
      <p class="meta">Task: <code>{item.task_id}</code></p>
    {/if}
    <pre>{payloadJson(item.payload)}</pre>
  {/if}
</div>

<style>
  .details {
    padding: 0.75rem 1rem;
    background: #f7f7f9;
    border-left: 3px solid #888;
    margin: 0.25rem 0;
  }
  dl {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.15rem 0.75rem;
    margin: 0 0 0.5rem 0;
    font-size: 0.85rem;
  }
  dt {
    font-weight: 600;
    color: #555;
  }
  h4 {
    margin: 0.5rem 0 0.25rem 0;
    font-size: 0.9rem;
  }
  pre {
    margin: 0;
    padding: 0.5rem;
    background: #fff;
    border: 1px solid #ddd;
    border-radius: 0.25rem;
    font-size: 0.8rem;
    overflow-x: auto;
    white-space: pre-wrap;
    word-wrap: break-word;
  }
  pre.text {
    font-family: inherit;
    font-size: 0.9rem;
  }
  .meta {
    margin: 0.25rem 0;
    font-size: 0.8rem;
    color: #555;
  }
  code {
    background: #eee;
    padding: 0.1rem 0.3rem;
    border-radius: 0.15rem;
    font-size: 0.85em;
  }
</style>
