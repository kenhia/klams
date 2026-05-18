# Data Model: klams Initial MVP

This document defines the entities the MVP persists, their fields and
relationships, validation rules, and the canonical `MemoryWrite`
pipeline type. SQL DDL is illustrative; the source of truth is
`migrations/`.

## Entities

### Fact (Postgres `facts`)

A typed, versioned, structured assertion about Ken, his machines, or a
task. Owns a strict identity by `(type, payload_hash)`.

| Field | Type | Notes |
|---|---|---|
| `id` | UUID PK | Generated server-side (v7). |
| `type` | TEXT NOT NULL | `UserFact` \| `TaskFact` \| `EnvFact` (extensible). |
| `payload` | JSONB NOT NULL | Type-specific structured content. |
| `payload_hash` | BYTEA NOT NULL | SHA-256 of `(type, canonical_json(payload))`. |
| `version` | INT NOT NULL DEFAULT 1 | Bumped on each upsert that changes content. |
| `source` | TEXT NOT NULL | `User` \| `Controller` \| `Task` \| `AgentProposal`. |
| `confidence` | REAL NOT NULL DEFAULT 1.0 | 0.0–1.0. MVP sets `1.0` for non-agent sources. |
| `decay_weight` | REAL NOT NULL DEFAULT 1.0 | Reserved; MVP leaves at `1.0`. |
| `use_count` | INT NOT NULL DEFAULT 0 | Incremented on retrieval (best-effort). |
| `last_used_at` | TIMESTAMPTZ NULL | Updated on retrieval (best-effort). |
| `created_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | |
| `updated_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | Updated on version bump. |

Indexes:

```sql
CREATE INDEX facts_type_idx       ON facts (type);
CREATE INDEX facts_source_idx     ON facts (source);
CREATE INDEX facts_created_at_idx ON facts (created_at);
CREATE INDEX facts_payload_gin    ON facts USING GIN (payload jsonb_path_ops);
CREATE UNIQUE INDEX facts_type_payload_hash_idx ON facts (type, payload_hash);
```

Validation:

- `type` MUST be a known variant.
- `payload` MUST be a JSON object (not array / scalar).
- `source` MUST be a known variant. Agent-sourced writes are accepted
  but flagged in metrics; schema-level validation per `type` is a
  Phase 2 deliverable.

State transitions:

- **Create**: `INSERT … ON CONFLICT (type, payload_hash) DO NOTHING
  RETURNING …`. If no row returned, it's an upsert path.
- **Upsert (content unchanged)**: no-op; existing row id returned.
- **Upsert (content changed → same hash → impossible by construction)**:
  N/A.
- **Upsert via explicit fact id with changed payload**: `version` += 1,
  `payload_hash` recomputed, `updated_at` = now(). Used by the
  controller when it wants to amend a known fact.

### Event (Postgres `events`)

Append-only record of something that happened.

| Field | Type | Notes |
|---|---|---|
| `id` | UUID PK | Generated server-side (v7). |
| `task_id` | UUID NULL | Optional grouping. |
| `category` | TEXT NOT NULL | `Execution` \| `Service` \| `Repo` \| etc. (free-form for MVP). |
| `payload` | JSONB NOT NULL | Free-form. |
| `source` | TEXT NOT NULL | Same enumeration as `facts.source`. |
| `created_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | |

Indexes:

```sql
CREATE INDEX events_task_id_idx     ON events (task_id) WHERE task_id IS NOT NULL;
CREATE INDEX events_category_idx    ON events (category);
CREATE INDEX events_created_at_idx  ON events (created_at);
```

Validation:

- `category` non-empty.
- `payload` MUST be a JSON object.
- No update, no delete (enforced at the API layer; the table has no
  endpoints that mutate existing rows).

### Knowledge item (Qdrant collection `knowledge_items`)

A chunk of text plus metadata, embedded into a 384-dim vector.

Vector: `Vec<f32>` of length 384 (per `BAAI/bge-small-en-v1.5`).

Payload (Qdrant point payload, JSON):

| Field | Type | Notes |
|---|---|---|
| `id` | string (UUID) | Mirrors Qdrant point id; surfaced in API. |
| `text` | string | Original chunk text, NFC-normalized. |
| `content_hash` | string (hex) | SHA-256 of normalized text. Indexed. |
| `source` | string | Same enumeration as Fact/Event. |
| `tags` | string[] | Optional. |
| `repo` | string \| null | Optional. |
| `file` | string \| null | Optional. |
| `machine` | string \| null | Optional. |
| `confidence` | number | 0.0–1.0. |
| `decay_weight` | number | Reserved; MVP `1.0`. |
| `use_count` | integer | Reserved. |
| `last_used_at` | RFC3339 \| null | Reserved. |
| `created_at` | RFC3339 | |
| `updated_at` | RFC3339 | |

Qdrant collection config (set at service startup, idempotent):

- `vector_size = 384`
- `distance = Cosine`
- payload indexes: `content_hash` (keyword), `source` (keyword),
  `tags` (keyword), `repo` (keyword), `machine` (keyword)
- `on_disk = true`

Validation:

- `text` length: 1..=8192 chars after normalization (FR error case
  for >8 KB).
- `tags` deduplicated, max 32 entries, each ≤ 64 chars.

## Pipeline type

```rust
// In `klams-types`
pub enum MemoryWrite {
    UpsertFact(UpsertFact),
    AppendEvent(AppendEvent),
    IndexKnowledge(IndexKnowledge),
}

pub struct UpsertFact {
    pub fact_type: FactType,
    pub payload: serde_json::Value,
    pub source: Source,
    pub explicit_id: Option<Uuid>, // present when amending a known fact
}

pub struct AppendEvent {
    pub task_id: Option<Uuid>,
    pub category: String,
    pub payload: serde_json::Value,
    pub source: Source,
}

pub struct IndexKnowledge {
    pub text: String,                // pre-normalization
    pub source: Source,
    pub tags: Vec<String>,
    pub repo: Option<String>,
    pub file: Option<String>,
    pub machine: Option<String>,
}
```

Flow:

1. API handler validates inbound DTO → constructs `MemoryWrite`.
2. For `UpsertFact`: the handler **awaits the write** (so it can
   return the canonical `id` and `version`) — implemented as a oneshot
   reply channel attached to the queue item.
3. For `AppendEvent` and `IndexKnowledge`: enqueue and return
   immediately with the assigned id.
4. Workers drain the queue, do dedupe + persist, increment metrics,
   and (for fact upserts) send the persisted row back via the oneshot.

When queue capacity is exceeded, the API returns 503 with
`Retry-After`, never blocks.

## Search result

```rust
pub struct SearchResults {
    pub query: String,
    pub results: Vec<SearchHit>,
    pub total: usize,
}

pub struct SearchHit {
    pub r#type: SearchType, // "fact" | "event" | "knowledge"
    pub id: Uuid,
    pub score: f32,         // [0.0, 1.0] normalized per-type
    pub preview: String,    // <=200 chars derived from payload/text
    pub payload: serde_json::Value, // truncated form of the source entity
}
```

Hybrid scoring for MVP:

- **knowledge**: Qdrant cosine similarity, returned as `score`.
- **fact / event**: Postgres `tsvector` match score from a generated
  column (`tsv = to_tsvector('english', payload::text)`), normalized
  to `[0, 1]` per result set.
- The merge step interleaves by per-type rank, capped at `top_k`.

This is intentionally simple; tuning is Phase 4.

## Health snapshot

```rust
pub struct HealthSnapshot {
    pub status: HealthStatus,    // Ok | Degraded | Down
    pub postgres: SubsystemStatus,
    pub qdrant: SubsystemStatus,
    pub embeddings: SubsystemStatus,
    pub queue: QueueStatus,
    pub version: String,
    pub uptime_seconds: u64,
}

pub struct QueueStatus {
    pub depth: usize,
    pub capacity: usize,
    pub workers: usize,
}
```

`/healthz` returns 200 when `status == Ok` (all subsystems `Ok`),
503 otherwise. Each `SubsystemStatus` includes a short `message` when
not `Ok`.
