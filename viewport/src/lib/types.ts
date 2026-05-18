/** DTOs mirroring `klams_types`. Keep in sync with
 *  `crates/klams-types/src/lib.rs` re-exports. */

export type Source = 'User' | 'Controller' | 'Task' | 'AgentProposal';
export type FactType = 'UserFact' | 'TaskFact' | 'EnvFact';
export type HealthState = 'Ok' | 'Degraded' | 'Down';

export interface Fact {
  id: string;
  fact_type: FactType;
  payload: unknown;
  payload_hash: string;
  source: Source;
  confidence: number;
  decay_weight: number;
  use_count: number;
  last_used_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface FactPage {
  items: Fact[];
  next_cursor: string | null;
}

export interface KlamsEvent {
  id: string;
  task_id: string | null;
  category: string;
  payload: unknown;
  source: Source;
  created_at: string;
}

export interface EventPage {
  items: KlamsEvent[];
  next_cursor: string | null;
}

export interface KnowledgeItem {
  id: string;
  text: string;
  content_hash: string;
  source: Source;
  tags: string[];
  repo: string | null;
  file: string | null;
  machine: string | null;
  confidence: number;
  decay_weight: number;
  use_count: number;
  last_used_at: string | null;
  created_at: string;
  updated_at: string;
}

export interface SearchHit {
  kind: 'fact' | 'event' | 'knowledge';
  id: string;
  score: number;
  preview: string;
}

export interface SearchResults {
  query: string;
  results: SearchHit[];
  total: number;
  degraded: boolean;
}

export interface SubsystemStatus {
  state: HealthState;
  message?: string;
}

export interface HealthSnapshot {
  status: HealthState;
  postgres: SubsystemStatus;
  qdrant: SubsystemStatus;
  embeddings: SubsystemStatus;
  queue: { depth: number; capacity: number; workers: number };
  version: string;
  uptime_seconds: number;
}

export interface ViewportConfig {
  klams_url: string;
  has_token: boolean;
  refresh_interval_seconds: number;
}
