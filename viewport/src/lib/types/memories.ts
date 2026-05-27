import type { PublicAuthorRef } from '../types';

export type ActivityMemoryKind = 'fact' | 'knowledge' | 'event';
export type ActivityMemoryState = 'live' | 'deleted' | 'all';

export type ActivityMemoryContent =
  | {
      kind: 'fact';
      type: string;
      payload: unknown;
    }
  | {
      kind: 'knowledge';
      text: string;
      source_path?: string;
      repo?: string;
    }
  | {
      kind: 'event';
      category: string;
      payload: unknown;
      task_id?: string;
    };

export type MemoryItem = {
  id: string;
  tags: string[];
  author: PublicAuthorRef;
  created_at: string;
  updated_at: string;
  state: 'live' | 'deleted';
  deleted_at?: string;
  deleted_by?: PublicAuthorRef;
  deleted_by_author_id?: string;
} & ActivityMemoryContent;

export interface ListMemoriesRequest {
  since?: string;
  until?: string;
  kinds?: ActivityMemoryKind[];
  state?: ActivityMemoryState;
  authors?: string[];
  limit?: number;
  cursor?: string;
}

export interface ListMemoriesResponse {
  memories: MemoryItem[];
  next_cursor: string | null;
}
