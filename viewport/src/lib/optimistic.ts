/** Optimistic update helper for Svelte stores.
 *
 *  Snapshots the current value, applies a predicted next value, then
 *  runs the async backend operation. On rejection the snapshot is
 *  restored and the error is re-thrown so callers can surface it.
 */

import { get, type Writable } from 'svelte/store';

export async function withOptimistic<T, R>(
  store: Writable<T>,
  prediction: (current: T) => T,
  op: () => Promise<R>
): Promise<R> {
  const snapshot = get(store);
  store.set(prediction(snapshot));
  try {
    return await op();
  } catch (err) {
    store.set(snapshot);
    throw err;
  }
}
