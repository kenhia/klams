<script lang="ts">
  import ContextPreview from '$lib/components/ContextPreview.svelte';
</script>

<h1>Context preview</h1>
<p class="muted">
  Inspect what <code>POST /memory/context</code> would return for a query and
  token budget. Tunes the slider with a 250 ms debounce; toggle to suppress
  summarized items and see only raw retrieval.
</p>

<ContextPreview />

<style>
  .muted { color: #888; font-size: 0.9rem; }
</style>
