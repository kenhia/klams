/** Typed wrappers around `@tauri-apps/api/core::invoke`.
 *
 *  Every wrapper sends arguments under an `args` envelope to match
 *  the backend command signatures defined in
 *  `viewport/src-tauri/src/commands/`.
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  EventPage,
  Fact,
  FactPage,
  HealthSnapshot,
  KlamsEvent,
  KnowledgeItem,
  SearchResults,
  ViewportConfig
} from './types';

export interface ListFactsArgs {
  fact_type?: string;
  source?: string;
  created_after?: string;
  created_before?: string;
  limit?: number;
  cursor?: string;
}

export interface ListEventsArgs {
  task_id?: string;
  category?: string;
  created_after?: string;
  created_before?: string;
  limit?: number;
  cursor?: string;
}

export interface SearchArgs {
  query: string;
  types?: ('fact' | 'event' | 'knowledge')[];
  filters?: unknown;
  top_k?: number;
}

export interface SetConfigArgs {
  klams_url?: string;
  bearer_token?: string;
  refresh_interval_seconds?: number;
}

export const api = {
  listFacts: (args: ListFactsArgs = {}) => invoke<FactPage>('list_facts', { args }),
  listEvents: (args: ListEventsArgs = {}) => invoke<EventPage>('list_events', { args }),
  searchUnified: (args: SearchArgs) => invoke<SearchResults>('search_unified', { args }),
  searchKnowledge: (args: SearchArgs) => invoke<SearchResults>('search_knowledge', { args }),
  getFact: (id: string) => invoke<Fact>('get_fact', { args: { id } }),
  getEvent: (id: string) => invoke<KlamsEvent>('get_event', { args: { id } }),
  getKnowledgeItem: (id: string) => invoke<KnowledgeItem>('get_knowledge_item', { args: { id } }),
  getHealth: () => invoke<HealthSnapshot>('get_health'),
  getConfig: () => invoke<ViewportConfig>('get_config'),
  setConfig: (args: SetConfigArgs) => invoke<ViewportConfig>('set_config', { args })
};
