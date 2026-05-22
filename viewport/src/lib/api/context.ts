/** Typed client for `POST /memory/context` (sprint 005).
 *
 *  The Tauri command `memory_context` proxies through
 *  `klams_client::Client::memory_context`, which provides the
 *  bearer/base-URL plumbing and returns a `ContextBundle` with
 *  per-section degradation indicators (`status: 'degraded'`) or
 *  surfaces a 503 + `Retry-After` when *every* source is
 *  unavailable. */

import { invoke } from '@tauri-apps/api/core';

export interface ContextRequest {
  query: string;
  token_budget: number;
  filters?: RetrievalFilters;
}

export interface RetrievalFilters {
  host?: string;
  type?: string;
  tag?: string;
  repo?: string;
  file?: string;
  source?: string;
  since?: string;
  until?: string;
}

export type ItemKind = 'raw' | 'summary' | 'digest';
export type SectionSource = 'raw' | 'summary' | 'mixed';
export type SectionStatus = 'ok' | 'degraded' | 'unavailable';
export type TokenEncoderId = 'cl100k_base' | 'chars_div4';

export interface ContextItem {
  kind: ItemKind;
  id: string;
  score: number;
  tokens: number;
  payload: unknown;
  source_ids?: string[];
}

export interface SectionMeta {
  count: number;
  tokens_spent: number;
  source: SectionSource;
  status: SectionStatus;
  degraded_reason?: string;
}

export interface ContextBundle {
  facts: ContextItem[];
  knowledge: ContextItem[];
  events: ContextItem[];
  total_spent: number;
  truncated: boolean;
  token_encoder: TokenEncoderId;
  sections: Record<string, SectionMeta>;
}

/** Server-side `ViewportError` shape surfaced by the Tauri command on
 *  503 (all sources unavailable). The `Retry-After` value from the
 *  HTTP response is propagated into the error message by
 *  `klams_client`; callers can substring-match `retry_after_seconds`
 *  to display a friendly hint. */
export interface ContextError {
  kind: 'not_configured' | 'network' | 'unauthorized' | 'server' | 'deserialization';
  status?: number;
  message?: string;
}

export const contextApi = {
  /** Fetch a `ContextBundle` for `req`. The Tauri command name is
   *  `memory_context`; arguments are passed under the `args`
   *  envelope to match the existing command surface. */
  fetch: (req: ContextRequest): Promise<ContextBundle> =>
    invoke<ContextBundle>('memory_context', { args: req })
};
