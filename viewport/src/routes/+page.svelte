<script lang="ts">
  import { health, lastRefresh, config } from '$lib/stores';

  const version = '0.1.0';
</script>

<h1>klams dashboard</h1>

<section>
  <h2>Connection</h2>
  <dl>
    <dt>Service URL</dt><dd>{$config?.klams_url ?? '(not configured)'}</dd>
    <dt>Viewport version</dt><dd>{version}</dd>
    <dt>Refresh interval</dt><dd>{$config?.refresh_interval_seconds ?? 10}s</dd>
    <dt>Last refresh</dt><dd>{$lastRefresh?.toLocaleTimeString() ?? '—'}</dd>
  </dl>
</section>

<section>
  <h2>Health</h2>
  {#if $health}
    <p>Aggregate: <strong>{$health.status}</strong> (uptime {$health.uptime_seconds}s, v{$health.version})</p>
    <table>
      <thead><tr><th>Subsystem</th><th>State</th><th>Message</th></tr></thead>
      <tbody>
        <tr><td>Postgres</td><td>{$health.postgres.state}</td><td>{$health.postgres.message ?? ''}</td></tr>
        <tr><td>Qdrant</td><td>{$health.qdrant.state}</td><td>{$health.qdrant.message ?? ''}</td></tr>
        <tr><td>Embeddings</td><td>{$health.embeddings.state}</td><td>{$health.embeddings.message ?? ''}</td></tr>
      </tbody>
    </table>
    <p>Queue: {$health.queue.depth}/{$health.queue.capacity} · workers {$health.queue.workers}</p>
  {:else}
    <p>Waiting for first health snapshot…</p>
  {/if}
</section>

<style>
  dl { display: grid; grid-template-columns: max-content 1fr; gap: 0.25rem 1rem; }
  dt { color: #555; }
  table { border-collapse: collapse; }
  th, td { padding: 0.25rem 0.75rem; border-bottom: 1px solid #eee; text-align: left; }
</style>
