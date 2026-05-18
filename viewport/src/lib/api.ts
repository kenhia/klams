/** Typed wrappers around `@tauri-apps/api/core::invoke`.
 *
 *  Every wrapper sends arguments under an `args` envelope to match
 *  the backend command signatures defined in
 *  `viewport/src-tauri/src/commands/`.
 */

import { invoke } from '@tauri-apps/api/core';
import type {
  Dissent,
  DissentPage,
  DissentStatus,
  EventPage,
  Fact,
  FactPage,
  FactType,
  HealthSnapshot,
  KlamsEvent,
  KnowledgeItem,
  SearchResults,
  Source,
  UpsertResult,
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

export interface ListDissentsArgs {
  fact_id?: string;
  status?: DissentStatus;
  source?: Source;
  created_after?: string;
  created_before?: string;
  limit?: number;
  cursor?: string;
}

export interface UpsertFactArgs {
  fact_type: FactType;
  payload: unknown;
  source: Source;
  explicit_id?: string;
  expected_version?: number;
}

export interface EditFactArgs {
  id: string;
  fact_type: FactType;
  payload: unknown;
  expected_version: number;
}

export interface PromoteDissentArgs {
  dissent_id: string;
  caller_source: Source;
  expected_version: number;
}

export interface DiscardDissentArgs {
  dissent_id: string;
  caller_source: Source;
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
  setConfig: (args: SetConfigArgs) => invoke<ViewportConfig>('set_config', { args }),
  listDissents: (args: ListDissentsArgs = {}) =>
    invoke<DissentPage>('list_dissents', { args }),
  getDissent: (id: string) => invoke<Dissent>('get_dissent', { args: { id } }),
  promoteDissent: (args: PromoteDissentArgs) => invoke<Fact>('promote_dissent', { args }),
  discardDissent: (args: DiscardDissentArgs) => invoke<Dissent>('discard_dissent', { args }),
  upsertFact: (args: UpsertFactArgs) => invoke<UpsertResult>('upsert_fact', { args }),
  editFact: (args: EditFactArgs) => invoke<UpsertResult>('edit_fact', { args }),
  deleteFact: (id: string) => invoke<void>('delete_fact', { args: { id } })
};
