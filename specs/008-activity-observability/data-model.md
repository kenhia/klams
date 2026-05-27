# Data Model — Activity & Observability (sprint 008)

This document captures the **non-DB** data-model changes for sprint 008.
There are **no schema migrations** — every query in this sprint operates
over columns and indexes that already exist after sprint 007.

References: [spec.md](./spec.md), [research.md](./research.md), [plan.md](./plan.md).

---

## 1. No schema changes

Sprint 008 introduces:

- **No new Postgres tables.**
- **No new Postgres columns.**
- **No new Qdrant collections or payload fields.**
- **No new migrations.**

The query layer relies on:

- `facts.created_at`, `facts.id`, `facts.author_id`, `facts.deleted_at`, `facts.deleted_by_author_id` (sprint 007).
- `events.created_at`, `events.id`, `events.author_id`, `events.payload` (sprint 003 + 007).
- `knowledge_items` Qdrant payload: `created_at`, `author_id`, `deleted_at`, `deleted_by_author_id` (sprint 005 + 007).

A pre-existing GIN index on `events.payload` (from sprint 003) backs the
`payload @> $1::jsonb` containment predicate used by `event_search`.

---

## 2. New query types (`klams-store::lib`)

### 2.1 `ListMemoriesQuery`

```rust
#[derive(Debug, Clone)]
pub struct ListMemoriesQuery {
    pub since: chrono::DateTime<chrono::Utc>,
    pub until: chrono::DateTime<chrono::Utc>,
    pub kinds: Vec<MemoryKindFilter>,        // empty = all
    pub state: MemoryStateFilter,             // Live / Deleted / All
    pub authors: Vec<uuid::Uuid>,             // empty = unrestricted
    pub limit: u32,                           // clamped 1..=200 at the handler
    pub cursor: Option<String>,               // opaque, see R-003
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryKindFilter { Fact, Knowledge, Event }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStateFilter { Live, Deleted, All }
```

**Validation** (performed by the handler **before** the store call):

- `since <= until` (otherwise `400 INVALID_WINDOW`).
- `(until - since) <= api.memories_max_window_days` days (otherwise `400 WINDOW_TOO_LARGE`).
- `limit` clamped to `1..=200`.
- `cursor`, when present, MUST decode (otherwise `400 INVALID_CURSOR`, reusing sprint 007's behavior).

### 2.2 `ListMemoriesRow`

```rust
#[derive(Debug, Clone)]
pub struct ListMemoriesRow {
    pub memory: klams_types::PublicMemory,
    pub state: MemoryStateOut,                            // Live or Deleted
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub deleted_by: Option<klams_types::PublicAuthorRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStateOut { Live, Deleted }
```

Identical to sprint 007's `AuthorMemoryRow` / `AuthorMemoryStateOut`,
just renamed for the cross-author surface. The two row types stay
distinct (rather than being unified) because the underlying queries
differ — author drilldown filters by `author_id`, cross-author does
not — and unifying would force one consumer through the other's
filter validation paths.

### 2.3 `EventSearchQuery`

```rust
#[derive(Debug, Clone)]
pub struct EventSearchQuery {
    pub author_ids: Vec<uuid::Uuid>,                  // empty = unrestricted
    pub categories: Vec<String>,                       // empty = unrestricted
    pub since: chrono::DateTime<chrono::Utc>,
    pub until: chrono::DateTime<chrono::Utc>,
    pub payload_match: Option<serde_json::Value>,      // JSONB containment, see R-010
    pub limit: u32,                                    // clamped 1..=500 at the tool surface
    pub order: EventOrder,                             // Desc (default) or Asc
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventOrder { Desc, Asc }
```

**Validation** (performed by the `event_search` tool **before** the store call):

- Same window cap rules as `ListMemoriesQuery` (R-002).
- `categories` entries MUST be non-empty strings.
- `payload_match`, when present, MUST be a JSON object (not a primitive or an array).
- `limit` clamped to `1..=500`.

---

## 3. `Store` trait additions

```rust
// In klams_store::lib.rs (additive — no existing method changes):
#[async_trait]
pub trait Store: ... {
    // existing methods unchanged ...

    async fn list_memories(
        &self,
        q: ListMemoriesQuery,
    ) -> StoreResult<(Vec<ListMemoriesRow>, Option<String>)>;

    async fn event_search(
        &self,
        q: EventSearchQuery,
    ) -> StoreResult<(Vec<klams_types::PublicMemory>, Option<String>)>;
}
```

Default impls in the trait return `(Vec::new(), None)` so existing
test doubles compile without modification (sprint 007's pattern).

---

## 4. Per-kind page fetchers

Sitting under the trait methods, each storage backend gains
narrowly-scoped fetchers consumed by the composite store:

### 4.1 PostgresStore (`klams_store::postgres`)

```rust
pub async fn list_memories_facts_page(
    &self,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    authors: &[Uuid],
    state: PostgresMemoryState,
    limit: u32,
    cursor: Option<(OffsetDateTime, Uuid)>,
) -> Result<(Vec<FactWithDeletion>, Option<(OffsetDateTime, Uuid)>), sqlx::Error>;

pub async fn list_memories_events_page(
    &self,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    authors: &[Uuid],
    limit: u32,
    cursor: Option<(OffsetDateTime, Uuid)>,
) -> Result<(Vec<Event>, Option<(OffsetDateTime, Uuid)>), sqlx::Error>;

pub async fn event_search_page(
    &self,
    q: &EventSearchQuery,
    cursor: Option<(OffsetDateTime, Uuid)>,
) -> Result<(Vec<Event>, Option<(OffsetDateTime, Uuid)>), sqlx::Error>;
```

All three use the same SQL shape as sprint 007's
`list_facts_by_author` / `list_events_by_author`, swapping the
`author_id = $1` predicate for an `(author_id = ANY($1) OR
cardinality($1) = 0)` form and adding `created_at >= $2 AND
created_at < $3`.

`event_search_page` adds `payload @> $4::jsonb` when
`q.payload_match` is `Some`.

### 4.2 QdrantStore (`klams_store::qdrant`)

```rust
pub async fn list_memories_knowledge_page(
    &self,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    authors: &[Uuid],
    state: QdrantMemoryStateFilter,
    limit: u32,
    cursor: Option<(DateTime<Utc>, Uuid)>,
) -> Result<(Vec<KnowledgePoint>, Option<(DateTime<Utc>, Uuid)>), QdrantError>;
```

Backed by a `scroll` call with payload filter:

```text
filter:
  must:
    - range: { key: "created_at", gte: $since, lt: $until }
  must_not:                                   # when state = Live
    - is_empty: { key: "deleted_at" } == false
  should: (optional)                          # when authors non-empty
    - match: { key: "author_id", value: $a }  (one per author)
order_by: created_at, desc
```

---

## 5. Configuration extension

```rust
// klams_types::config::ApiConfig (additive, backward-compatible):
pub struct ApiConfig {
    // ... existing fields ...
    #[serde(default = "default_memories_max_window_days")]
    pub memories_max_window_days: u32,
}

fn default_memories_max_window_days() -> u32 { 30 }
```

**TOML example** (in `klams.toml` — no change required for existing deployments):

```toml
[api]
# memories_max_window_days = 30   # default; uncomment to override
```

---

## 6. Memory projection — surfacing soft-delete metadata

The wire shape returned by `GET /v1/memories` carries the optional
deletion metadata exactly like sprint 007's `AuthorMemoryWire`:

```rust
#[derive(Debug, Serialize)]
pub struct MemoriesPageItem {
    #[serde(flatten)]
    pub memory: serde_json::Value,    // serialized PublicMemory
    pub state: &'static str,          // "live" | "deleted"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_by: Option<klams_types::PublicAuthorRef>,
}

#[derive(Debug, Serialize)]
pub struct MemoriesPage {
    pub memories: Vec<MemoriesPageItem>,
    pub next_cursor: Option<String>,
}
```

For **live** rows: `state = "live"`, no `deleted_at`, no `deleted_by`
keys present in JSON output (per `skip_serializing_if`).

For **deleted** rows (only returned when `state ∈ {deleted, all}`):
`state = "deleted"`, `deleted_at` populated, `deleted_by` populated
from a `bulk_fetch_authors` lookup on the row's
`deleted_by_author_id`.

The base `PublicMemory` shape itself is **unchanged** from sprint 007.

`event_search` returns `Vec<PublicMemory>` directly (no wrapper), because
events have no deletion state to surface.

---

## 7. State transitions

No new state transitions. Sprint 007's diagrams in
[specs/007-mcp-server/data-model.md §8](../007-mcp-server/data-model.md#8-state-transitions)
apply unchanged.

The cross-author listing surface reads existing state; it does not
mutate it.

---

## 8. Cursor wire format

Identical to sprint 007's encoding:

```text
base64_url(section ":" ts_nanos ":" uuid)
```

Decoded as `(String, i128, Uuid)` by the existing `decode_memory_cursor`
helper in `crates/klams-store/src/composite.rs`.

For `event_search`, `section` is always `"e"`. For `list_memories`,
`section ∈ {"f","k","e"}` per R-003.

---

## 9. Cross-reference matrix

| FR | Affected types / surfaces |
|----|---------------------------|
| FR-001..005 | `EventSearchQuery`, `Store::event_search`, `tools/event_search.rs` |
| FR-006..011 | `ListMemoriesQuery`, `Store::list_memories`, `handlers/memories.rs`, `MemoriesPage(Item)` |
| FR-009      | `ApiConfig.memories_max_window_days`, `WINDOW_TOO_LARGE` error code |
| FR-012..015 | viewport `/activity` route + `lib/types/memories.ts` + Tauri `list_memories` command |
| FR-016..018 | `deploy/grafana/klams.json` panels, `deploy/prometheus/prometheus.yml` (new) |
| FR-019..022 | `tools/bench/` crate, `specs/008-activity-observability/perf-baseline.md`, `README.md` link |

> **Corpus sizing rationale (FR-019)**: 10k facts + 50k knowledge items mirrors the test-environment scale targeted in sprint 007 and is the floor for credible comparison; future runs against the homelab `kubs0` store must meet or exceed this corpus before their numbers are comparable to the checked-in baseline.
