/** Svelte 5 stores for global app state.
 *
 *  `health` is fed by the `klams://health` Tauri event emitted by
 *  the background poll in [`commands/health.rs`](../../src-tauri/src/commands/health.rs).
 *  `config` mirrors the persisted viewport config and is refreshed
 *  after `set_config` calls. */

import { writable } from 'svelte/store';
import { listen } from '@tauri-apps/api/event';
import type { HealthSnapshot, ViewportConfig } from './types';
import { api } from './api';

export const health = writable<HealthSnapshot | null>(null);
export const lastRefresh = writable<Date | null>(null);
export const config = writable<ViewportConfig | null>(null);
/** Increment after any canonical write so subscribers (e.g. the
 *  pending-dissents nav badge) can refresh. */
export const mutationCounter = writable<number>(0);
export const pendingDissentCount = writable<number>(0);

let started = false;

export async function startSubscriptions(): Promise<void> {
  if (started) return;
  started = true;

  await listen<HealthSnapshot>('klams://health', (e) => {
    health.set(e.payload);
    lastRefresh.set(new Date());
  });
  await listen<ViewportConfig>('klams://config-changed', (e) => {
    config.set(e.payload);
  });

  try {
    config.set(await api.getConfig());
  } catch {
    /* not configured yet; UI will surface the empty state */
  }
  try {
    const snap = await api.getHealth();
    health.set(snap);
    lastRefresh.set(new Date());
  } catch {
    /* leave null until first event arrives */
  }
}

export async function refreshConfig(): Promise<void> {
  config.set(await api.getConfig());
}

/** Re-fetch the pending-dissent count for the nav badge. Safe to
 *  call from any mutation site; errors are swallowed. */
export async function refreshPendingDissents(): Promise<void> {
  try {
    const page = await api.listDissents({ status: 'pending', limit: 200 });
    // Badge shows the visible count + "+" when the page is full.
    pendingDissentCount.set(page.items.length);
  } catch {
    /* leave previous value */
  }
}
