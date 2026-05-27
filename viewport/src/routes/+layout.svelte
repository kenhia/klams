<script lang="ts">
  import { onMount } from 'svelte';
  import {
    health,
    config,
    startSubscriptions,
    refreshConfig,
    pendingDissentCount,
    refreshPendingDissents,
    mutationCounter
  } from '$lib/stores';
  import { api } from '$lib/api';

  let { children } = $props();
  let showSettings = $state(false);
  let urlField = $state('');
  let tokenField = $state('');
  let intervalField = $state(10);
  let saveMessage = $state<string | null>(null);

  onMount(() => {
    void startSubscriptions();
    void refreshPendingDissents();
  });

  // Re-poll dissent count whenever a mutation completes.
  $effect(() => {
    $mutationCounter;
    void refreshPendingDissents();
  });

  $effect(() => {
    if (showSettings && $config) {
      urlField = $config.klams_url;
      intervalField = $config.refresh_interval_seconds;
    }
  });

  async function save() {
    saveMessage = null;
    try {
      await api.setConfig({
        klams_url: urlField,
        bearer_token: tokenField || undefined,
        refresh_interval_seconds: intervalField
      });
      await refreshConfig();
      saveMessage = 'Saved.';
      tokenField = '';
    } catch (e) {
      saveMessage = `Failed: ${String(e)}`;
    }
  }

  const dotColor = $derived(
    $health == null
      ? '#888'
      : $health.status === 'Ok'
        ? '#3a3'
        : $health.status === 'Degraded'
          ? '#c80'
          : '#c33'
  );
</script>

<header>
  <nav>
    <a href="/">Dashboard</a>
    <a href="/facts">Facts</a>
    <a href="/events">Events</a>
    <a href="/knowledge">Knowledge</a>
    <a href="/authors">Authors</a>
    <a href="/activity">Activity</a>
    <a href="/preview">Preview</a>
    <a href="/dissents" class="dissents-link">
      Dissents{#if $pendingDissentCount > 0}<span class="badge">{$pendingDissentCount}</span>{/if}
    </a>
  </nav>
  <div class="status">
    <span class="dot" style:background={dotColor}></span>
    <span>{$health?.status ?? 'connecting'}</span>
    <button onclick={() => (showSettings = true)} aria-label="Settings">⚙</button>
  </div>
</header>

<main>{@render children()}</main>

{#if showSettings}
  <div class="modal-backdrop" onclick={() => (showSettings = false)} role="presentation"></div>
  <div class="modal" role="dialog" aria-modal="true">
    <h2>Settings</h2>
    <label>
      klams URL
      <input type="url" bind:value={urlField} placeholder="http://kubs0:7777" />
    </label>
    <label>
      Bearer token
      <input
        type="password"
        bind:value={tokenField}
        placeholder={$config?.has_token ? 'token stored — leave blank to keep' : 'paste token here'}
      />
    </label>
    <label>
      Refresh interval (seconds)
      <input type="number" bind:value={intervalField} min="1" max="600" />
    </label>
    <div class="actions">
      <button onclick={() => (showSettings = false)}>Close</button>
      <button onclick={save}>Save</button>
    </div>
    {#if saveMessage}<p class="msg">{saveMessage}</p>{/if}
  </div>
{/if}

<style>
  :global(body) {
    font-family: system-ui, sans-serif;
    margin: 0;
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 0.5rem 1rem;
    border-bottom: 1px solid #ddd;
    background: #f7f7f7;
  }
  nav a {
    margin-right: 1rem;
    text-decoration: none;
    color: #246;
  }
  .dissents-link .badge {
    display: inline-block;
    margin-left: 0.3rem;
    background: #b35;
    color: white;
    border-radius: 9px;
    padding: 0 0.4rem;
    font-size: 0.75rem;
    min-width: 1.2rem;
    text-align: center;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    display: inline-block;
  }
  main {
    padding: 1rem;
  }
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.3);
  }
  .modal {
    position: fixed;
    top: 10%;
    left: 50%;
    transform: translateX(-50%);
    background: #fff;
    padding: 1.5rem;
    border-radius: 8px;
    min-width: 360px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.2);
  }
  .modal label {
    display: block;
    margin-bottom: 0.75rem;
  }
  .modal input {
    display: block;
    width: 100%;
    padding: 0.4rem;
    margin-top: 0.2rem;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
  .msg {
    margin-top: 0.5rem;
    color: #246;
  }
</style>
