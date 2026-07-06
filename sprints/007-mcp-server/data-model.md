# Data Model — MCP Memory Server (sprint 007)

This document captures the additive data-model changes for sprint 007.
All changes are **additive** — no existing column is renamed, dropped,
or repurposed. Migrations are forward-only.

References: [spec.md](./spec.md), [research.md](./research.md).

---

## 1. New table: `authors`

```sql
CREATE TABLE authors (
    id              UUID PRIMARY KEY,
    agent_name      TEXT NOT NULL,
    model           TEXT,
    session_title   TEXT,
    repo            TEXT,
    client_app      TEXT,
    client_version  TEXT,
    extra           JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_authors_agent_name ON authors (agent_name);
CREATE INDEX idx_authors_last_seen_at ON authors (last_seen_at DESC);
```

**Fields**:

- `id` — server-issued UUID v7 (time-ordered for natural pagination).
- `agent_name` — required; small free-form string (e.g., `"GHCP"`, `"controller"`).
- `model` — optional; populated when the agent knows its backing model.
- `session_title`, `repo`, `client_app`, `client_version` — optional; populated by `register_author` arguments.
- `extra` — JSONB escape hatch for future fields without a migration.
- `created_at` — set once at insertion.
- `last_seen_at` — updated by the server on every authenticated MCP call that references the author (FR-005).

**Validation rules**:

- `agent_name` MUST be non-empty (validated server-side in `register_author`).
- `repo`, when present, MUST be an absolute path (validated server-side).
- `extra` MUST be ≤ 16 KiB serialized (mirrors `EnvFact` cap from sprint 004).

**System author**:

A single fixed row is seeded by the migration:

```sql
INSERT INTO authors (id, agent_name, model, client_app, created_at, last_seen_at)
VALUES (
    '00000000-0000-7000-8000-000000000001'::uuid,
    'system',
    NULL,
    'klams-service',
    now(),
    now()
)
ON CONFLICT (id) DO NOTHING;
```

This UUID is exposed in code as `klams_types::SYSTEM_AUTHOR_ID`. All
pre-MCP rows are backfilled to reference it (FR-007).

---

## 2. Existing table: `facts` — additive columns

```sql
ALTER TABLE facts
    ADD COLUMN author_id            UUID,
    ADD COLUMN deleted_at           TIMESTAMPTZ,
    ADD COLUMN deleted_by_author_id UUID;

UPDATE facts SET author_id = '00000000-0000-7000-8000-000000000001'::uuid
    WHERE author_id IS NULL;

ALTER TABLE facts
    ALTER COLUMN author_id SET NOT NULL,
    ADD CONSTRAINT facts_author_id_fkey
        FOREIGN KEY (author_id) REFERENCES authors(id),
    ADD CONSTRAINT facts_deleted_by_fkey
        FOREIGN KEY (deleted_by_author_id) REFERENCES authors(id);

CREATE INDEX idx_facts_author_id ON facts (author_id);
CREATE INDEX idx_facts_deleted_at ON facts (deleted_at) WHERE deleted_at IS NOT NULL;
```

**Semantics**:

- `author_id` — non-null, every row attributed.
- `deleted_at` — null = live; non-null = soft-deleted at this UTC timestamp.
- `deleted_by_author_id` — set together with `deleted_at`; identifies the author whose `memory_delete` triggered the soft delete. The same column is **not** updated on restore (the original delete attribution is preserved; restore is recorded separately in metrics).

**Default search filter** (applied in every read path unless explicitly overridden by an admin tool):

```sql
WHERE deleted_at IS NULL
```

**Restore semantics** (admin):

```sql
UPDATE facts SET deleted_at = NULL, deleted_by_author_id = NULL WHERE id = $1;
```

**Hard delete semantics** (admin):

```sql
DELETE FROM facts WHERE id = $1;
```

---

## 3. Existing table: `events` — additive column only

```sql
ALTER TABLE events ADD COLUMN author_id UUID;

UPDATE events SET author_id = '00000000-0000-7000-8000-000000000001'::uuid
    WHERE author_id IS NULL;

ALTER TABLE events
    ALTER COLUMN author_id SET NOT NULL,
    ADD CONSTRAINT events_author_id_fkey FOREIGN KEY (author_id) REFERENCES authors(id);

CREATE INDEX idx_events_author_id ON events (author_id);
```

**No soft-delete columns** (FR-015, R-006). Events are append-only.

---

## 4. Qdrant `knowledge_items` collection — payload extensions

The collection schema in Qdrant is schemaless; new payload fields are
added by writes, not migrations. The store layer is updated to:

- **Write path**: every new knowledge point includes `author_id` (string-formatted UUID) in payload. A startup-time backfill scans existing points and stamps `SYSTEM_AUTHOR_ID` where the field is missing.
- **Soft delete**: `memory_delete` on a knowledge point sets payload fields `deleted_at` (ISO-8601 UTC string) and `deleted_by_author_id` (UUID string).
- **Default search filter**: every search adds `must_not: [{ key: "deleted_at", match: { except: null } }]`.
- **Restore**: payload update removing `deleted_at` and `deleted_by_author_id`.
- **Hard delete**: standard Qdrant point delete-by-id.

**Backfill task**: a one-shot operation runs on first startup after the
migration; tracked by a sentinel row in `facts.payload` (`{"_backfill": "qdrant_authors", "version": 1}`) so reruns are no-ops.

---

## 5. Configuration extension: `klams-types::AuthConfig`

```rust
// Pre-sprint-007 (existing):
pub struct AuthConfig {
    pub bearer_token: String,
}

// Post-sprint-007:
pub struct AuthConfig {
    /// Legacy single-token form. When non-empty, materialized at load
    /// time into one `TokenGrant` with all scopes set.
    #[serde(default)]
    pub bearer_token: String,

    /// Multi-token form. Each entry carries its own scope set.
    #[serde(default)]
    pub tokens: Vec<TokenGrantConfig>,
}

pub struct TokenGrantConfig {
    pub token: String,
    pub scopes: Vec<Scope>,
    /// Optional human label for logs and metrics token-hash diagnostics.
    pub label: Option<String>,
}

pub enum Scope { Read, Write, Admin }
```

**TOML example** (in `klams.toml`):

```toml
[auth]
# Legacy form (still supported, grants all scopes):
# bearer_token = "..."

[[auth.tokens]]
token = "viewport-readonly-..."
scopes = ["read"]
label = "viewport"

[[auth.tokens]]
token = "ghcp-write-..."
scopes = ["read", "write"]
label = "ghcp"

[[auth.tokens]]
token = "admin-..."
scopes = ["read", "write", "admin"]
label = "ken-admin"
```

**Validation**:

- At least one of `bearer_token` (non-empty) or `tokens` (non-empty) MUST be present.
- Token strings MUST be ≥ 16 characters (loose entropy floor — operator's responsibility for real entropy).
- `scopes` MUST be non-empty.

---

## 6. Memory projection (over-the-wire)

This is **not** a database type; it is the JSON shape returned by MCP
tools and (in slightly extended form) by the new viewport REST routes.
Definitive schemas live in [contracts/](./contracts/).

```rust
// In klams-types (added):
pub struct PublicMemory {
    pub id: Uuid,
    pub kind: MemoryKind, // "fact" | "knowledge" | "event"
    pub content: PublicMemoryContent, // per-kind body — see below
    pub tags: Vec<String>,
    pub author: PublicAuthorRef,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum PublicMemoryContent {
    Fact { r#type: String, payload: serde_json::Value },
    Knowledge { text: String, source_path: Option<String>, repo: Option<String> },
    Event { category: String, payload: serde_json::Value, task_id: Option<Uuid> },
}

pub struct PublicAuthorRef {
    pub agent_name: String,
    pub model: Option<String>,
    pub repo: Option<String>,
}
```

**Omitted from every projection** (enforced by the projection layer being the only public-facing serialization path; the internal `Fact`/`Event`/`KnowledgeItem` types are never returned over MCP or REST author endpoints):

- `version` (optimistic concurrency)
- `decay_weight`, `confidence`, `use_count`, `last_used_at`
- raw embedding vector
- internal `source` trust tier (`User`/`Controller`/`Task`/`AgentProposal`)
- internal database identifiers other than the public `id` UUID
- soft-deletion bookkeeping (`deleted_at`, `deleted_by_author_id`) — clients see deleted items only via admin tools, where these fields appear in the admin response shape (see `contracts/`)

---

## 7. New constants

```rust
// klams-types::lib.rs
pub const SYSTEM_AUTHOR_ID: Uuid =
    uuid::uuid!("00000000-0000-7000-8000-000000000001");
```

---

## 8. State transitions

### Fact / Knowledge item lifecycle

```text
            ┌────────────────┐
            │ written        │  (deleted_at = NULL)
            └───────┬────────┘
                    │ memory_delete(id)
                    ▼
            ┌────────────────┐
            │ soft-deleted   │  (deleted_at = T, deleted_by_author_id = A)
            └───────┬────────┘
                    │ memory_admin_restore(id)
                    │       ↑
                    │       │ memory_admin_hard_delete(id)
                    ▼       │
            ┌────────────────┐
            │ gone           │  (row removed from Postgres / Qdrant)
            └────────────────┘
```

Invariants:

- `deleted_at IS NULL` ⟺ row is live in `memory_search` default results.
- A row that has been hard-deleted leaves no trace in `memory_admin_list_deleted` (FR-016 list is for soft-deleted items only).
- `events` rows never transition out of "written" via MCP.

### Author lifecycle

```text
register_author → row inserted → last_seen_at touched on every
                                  authenticated MCP call referencing this id
```

No deletion path for authors in v1. (Cleanup of stale authors is a
future ops concern, not an MCP capability.)

---

## 9. Dedupe behavior

`memory_add` for facts dispatches to the existing `Worker` pipeline,
which already performs `(type, canonical_payload_hash)` dedupe. The
MCP path is a thin wrapper:

1. Validate `author_id` exists.
2. Build `MemoryWrite::UserFactUpsert` (or appropriate variant) with
   `author_id` attached.
3. Enqueue.
4. Return the resulting fact's public projection (including the
   pre-existing fact's id if dedupe matched).

Same behavior for knowledge: existing content-hash dedupe applies.

---

## 10. Migration ordering

```text
migrations/
  0005_authors_table.sql                # CREATE TABLE authors + seed system author
  0006_facts_author_and_soft_delete.sql # ADD COLUMNs to facts, backfill, NOT NULL + FK
  0007_events_author.sql                # ADD COLUMN to events, backfill, NOT NULL + FK
```

The Qdrant payload backfill is a **runtime** operation, not a SQL
migration; see §4. It runs idempotently on every startup until the
sentinel marks completion.

---

## Cross-reference matrix

| FR | Tables/columns touched |
|----|------------------------|
| FR-004 | `authors` table |
| FR-005 | `authors.last_seen_at` |
| FR-006 | `authors.id` FK from `facts`, `events`, Qdrant payload |
| FR-007 | Backfill via system author seed |
| FR-008 | `PublicMemory` projection (no DB change beyond above) |
| FR-013 | `facts.deleted_at`, Qdrant payload `deleted_at` |
| FR-014 | Idempotent UPDATE in service layer; no DB change |
| FR-015 | `events` deliberately omits soft-delete cols |
| FR-016 | Admin tools read inverse filter; reuse same columns |
| FR-017 | `AuthConfig.tokens` |
| FR-018 | `AuthConfig.bearer_token` preserved |
| FR-024 | Postgres views/joins backing `GET /v1/authors*` (no new tables) |
