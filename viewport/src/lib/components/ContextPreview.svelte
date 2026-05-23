<!--
  Sprint 005 (T050) — Context-preview pane.

  Lets the operator inspect what `/memory/context` would return for
  a given query and token budget without touching CLI tooling.
  The token-budget slider debounces fetches at 250 ms
  (research.md D-009). A raw-vs-summarized toggle re-fetches by
  re-running the same request (server picks raw or summarized per
  section based on size + readiness).

  Section status pills surface `degraded`/`unavailable` from the
  server, and `503` from the API (all sources down) maps to a
  prominent error banner so the operator knows to retry.
-->
<script lang="ts">
  import { contextApi, type ContextBundle } from '$lib/api/context';

  let query = $state('');
  let tokenBudget = $state(2048);
  let showSummarized = $state(true);
  let bundle = $state<ContextBundle | null>(null);
  let loading = $state(false);
  let error = $state<string | null>(null);

  let debounce: ReturnType<typeof setTimeout> | null = null;

  async function run() {
    if (!query.trim()) {
      bundle = null;
      error = null;
      return;
    }
    loading = true;
    error = null;
    try {
      bundle = await contextApi.fetch({
        query: query.trim(),
        token_budget: tokenBudget
      });
    } catch (e) {
      const err = e as { kind?: string; status?: number; message?: string };
      error =
        err.status === 503
          ? `All retrieval sources unavailable. ${err.message ?? ''}`
          : (err.message ?? String(e));
      bundle = null;
    } finally {
      loading = false;
    }
  }

  function scheduleRun() {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => void run(), 250);
  }

  function onSliderInput(e: Event) {
    tokenBudget = Number((e.target as HTMLInputElement).value);
    scheduleRun();
  }

  function onToggle() {
    showSummarized = !showSummarized;
    void run();
  }

  function pillClass(status: string) {
    return status === 'ok' ? 'pill ok' : status === 'degraded' ? 'pill warn' : 'pill err';
  }

  const sectionOrder = ['facts', 'knowledge', 'events'] as const;
  type Section = (typeof sectionOrder)[number];

  function items(section: Section) {
    if (!bundle) return [];
    return bundle[section];
  }

  function filteredItems(section: Section) {
    const all = items(section);
    if (showSummarized) return all;
    return all.filter((it) => it.kind === 'raw');
  }
</script>

<section class="ctx-preview">
  <header>
    <form onsubmit={(e) => { e.preventDefault(); void run(); }}>
      <label>
        Query
        <input
          type="text"
          bind:value={query}
          placeholder="e.g. how is GPU drivers handled on kai"
          oninput={scheduleRun}
        />
      </label>
      <label>
        Budget: {tokenBudget} tok
        <input
          type="range"
          min="256"
          max="8192"
          step="128"
          value={tokenBudget}
          oninput={onSliderInput}
        />
      </label>
      <label class="toggle">
        <input type="checkbox" checked={showSummarized} onchange={onToggle} />
        show summaries
      </label>
      <button type="submit" disabled={loading || !query.trim()}>Refresh</button>
    </form>
  </header>

  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if loading && !bundle}
    <p class="muted">Loading…</p>
  {/if}

  {#if bundle}
    <p class="totals">
      Encoder <code>{bundle.token_encoder}</code> ·
      total <strong>{bundle.total_spent}</strong> /
      {tokenBudget} tok
      {#if bundle.truncated}<span class="pill warn">truncated</span>{/if}
    </p>

    {#each sectionOrder as section (section)}
      {@const meta = bundle.sections[section]}
      {#if meta}
        <article class="section">
          <h3>
            {section}
            <span class={pillClass(meta.status)}>{meta.status}</span>
            <span class="muted">
              {meta.count} item{meta.count === 1 ? '' : 's'} ·
              {meta.tokens_spent} tok · source: {meta.source}
            </span>
            {#if meta.degraded_reason}
              <span class="reason">— {meta.degraded_reason}</span>
            {/if}
          </h3>
          <ul>
            {#each filteredItems(section) as item (item.id)}
              <li>
                <span class="kind">{item.kind}</span>
                <span class="score">{item.score.toFixed(3)}</span>
                <span class="tokens">{item.tokens} tok</span>
                <code class="id">{item.id.slice(0, 8)}</code>
                <pre>{JSON.stringify(item.payload, null, 0).slice(0, 240)}</pre>
              </li>
            {/each}
          </ul>
        </article>
      {/if}
    {/each}
  {/if}
</section>

<style>
  .ctx-preview {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }
  form {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    align-items: end;
  }
  form label {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.85rem;
  }
  form label.toggle {
    flex-direction: row;
    align-items: center;
    gap: 0.4rem;
  }
  input[type='text'] {
    min-width: 22rem;
  }
  input[type='range'] {
    min-width: 12rem;
  }
  .pill {
    display: inline-block;
    padding: 0.1rem 0.5rem;
    border-radius: 0.75rem;
    font-size: 0.75rem;
    margin-left: 0.4rem;
  }
  .pill.ok { background: #265; color: #cfe; }
  .pill.warn { background: #762; color: #fed; }
  .pill.err { background: #722; color: #fdd; }
  .error { color: #c33; }
  .muted { color: #888; }
  .totals { font-size: 0.9rem; }
  .section { border: 1px solid #333; border-radius: 0.5rem; padding: 0.5rem 0.75rem; }
  .section h3 { margin: 0 0 0.4rem; font-size: 1rem; }
  ul { list-style: none; padding: 0; margin: 0; }
  li {
    display: grid;
    grid-template-columns: 5rem 4rem 4rem 6rem 1fr;
    gap: 0.5rem;
    padding: 0.25rem 0;
    border-top: 1px solid #222;
    align-items: center;
  }
  .kind { font-size: 0.75rem; color: #8af; }
  .score { font-variant-numeric: tabular-nums; }
  .tokens { font-variant-numeric: tabular-nums; color: #888; }
  .id { font-family: monospace; font-size: 0.75rem; color: #888; }
  pre {
    margin: 0;
    font-family: monospace;
    font-size: 0.75rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .reason { color: #c80; font-size: 0.8rem; margin-left: 0.4rem; }
</style>
