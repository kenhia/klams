/** DTOs mirroring `klams_types`. Keep in sync with
 *  `crates/klams-types/src/lib.rs` re-exports. */

export type Source = 'User' | 'Controller' | 'Task' | 'AgentProposal';
export type FactType = 'UserFact' | 'TaskFact' | 'EnvFact';
export type HealthState = 'Ok' | 'Degraded' | 'Down';
export type DissentStatus = 'pending' | 'promoted' | 'discarded' | 'orphaned';

export interface Fact {
  id: string;
  fact_type: FactType;
  payload: unknown;
  payload_hash: string;
  version: number;
  source: Source;
  confidence: number;
  decay_weight: number;
  use_count: number;
  /** Pending-dissent count maintained by Postgres triggers (sprint 002). */
  dissent_count: number;
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

export interface Dissent {
  id: string;
  fact_id: string;
  proposed_payload: unknown;
  source: Source;
  status: DissentStatus;
  submitted_at: string;
  last_seen_at: string;
  submission_count: number;
  resolved_at: string | null;
  resolved_by_source: Source | null;
}

export interface DissentPage {
  items: Dissent[];
  next_cursor: string | null;
}

/** Discriminated union mirroring `klams_types::FactWriteOutcome`.
 *  Serde tag is `outcome`, variants are snake_case. */
export type UpsertResult =
  | { outcome: 'persisted'; fact: Fact }
  | { outcome: 'dissented'; dissent_id: string; fact_id: string }
  | { outcome: 'version_conflict'; current_version: number; fact_id: string };

/** Provenance fields surfaced by [`ProvenancePanel`](./ProvenancePanel.svelte).
 *  Derivable from a `Fact` or `KnowledgeItem`. */
export interface ProvenanceBundle {
  source: Source;
  confidence: number;
  decay_weight: number;
  use_count: number;
  last_used_at: string | null;
  created_at: string;
  updated_at: string;
  version?: number;
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

/* ---- Sprint 007: /v1/authors drilldown ----------------------- */

export interface PublicAuthorRef {
  agent_name: string;
  model?: string;
  repo?: string;
}

export interface AuthorCounts {
  writes: number;
  events: number;
  soft_deletes: number;
  restores_received: number;
}

export interface PublicAuthor {
  id: string;
  agent_name: string;
  model?: string;
  session_title?: string;
  repo?: string;
  client_app?: string;
  client_version?: string;
  created_at: string;
  last_seen_at: string;
  counts: AuthorCounts;
}

export interface AuthorPage {
  authors: PublicAuthor[];
  next_cursor: string | null;
}

export type MemoryKind = 'fact' | 'knowledge' | 'event';

export type PublicMemoryContent =
  | { kind: 'fact'; type: string; payload: unknown }
  | { kind: 'knowledge'; text: string; source_path?: string; repo?: string }
  | { kind: 'event'; category: string; payload: unknown; task_id?: string };

export type AuthorMemoryState = 'live' | 'deleted';

/** Row returned by `GET /v1/authors/{id}/memories`. The PublicMemory
 *  fields (`id`, `kind`, content discriminated union, `tags`, `author`,
 *  timestamps) are flattened into the row alongside `state` and the
 *  optional soft-delete fields. */
export type AuthorMemoryRow = {
  id: string;
  tags: string[];
  author: PublicAuthorRef;
  created_at: string;
  updated_at: string;
  state: AuthorMemoryState;
  deleted_at?: string;
  deleted_by?: PublicAuthorRef;
} & PublicMemoryContent;

export interface AuthorMemoriesPage {
  memories: AuthorMemoryRow[];
  next_cursor: string | null;
}
