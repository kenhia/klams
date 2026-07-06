# Data Model — Sprint 009

## 1. `TokenGrantConfig` extension

**Location**: `crates/klams-types/src/auth.rs`

**Before**:

```rust
pub struct TokenGrantConfig {
    pub token: SecretString,
    pub scopes: Vec<Scope>,
    pub note: Option<String>,
}
```

**After**:

```rust
pub struct TokenGrantConfig {
    pub token: SecretString,
    pub scopes: Vec<Scope>,
    pub note: Option<String>,
    /// Resolved to an `Author` at service startup. When `None`,
    /// the grant attributes its writes to the seeded `system`
    /// author (back-compat for tokens issued before sprint 009).
    pub agent_name: Option<String>,
}
```

**Validation** (at deserialize time):

- If `Some(name)`, name must match the existing
  `Author::validate_agent_name` rules (lowercase ASCII, `[a-z0-9_-]`,
  length 2–64).
- Empty string `Some("")` is rejected.

**Failure mode**: invalid `agent_name` → `ConfigError::InvalidAgentName`
at startup; service refuses to bind.

## 2. `AuthorBinding` cache

**Location**: `crates/klams-service/src/auth.rs` (new module or
existing one; see plan).

```rust
#[derive(Clone)]
pub struct AuthorBinding {
    pub author_id: Uuid,
    pub agent_name: String,
}

pub type AuthorBindings = Arc<HashMap<TokenBytes, AuthorBinding>>;
```

Built once at service startup, inserted into axum app state, attached
to each authenticated request as a request extension by the auth
middleware. REST handlers extract it via `Extension<AuthorBinding>`.

## 3. Pipeline structs

**Location**: `crates/klams-types/src/pipeline.rs`

Each of these gains a required `author_id: Uuid` field:

- `UpsertFact`
- `AppendEvent`
- `IndexKnowledge`

The `MemoryWrite` enum signature is unchanged — the variants already
wrap these structs.

## 4. Store layer additions

**Location**: `crates/klams-store/src/postgres.rs`,
`crates/klams-store/src/qdrant.rs`

- `index_knowledge_with_author` (NEW, mirrors the existing
  `upsert_fact_with_author` / `append_event_with_author` shape):
  - Takes `&IndexKnowledge` + `author_id: Uuid`.
  - Stamps Qdrant payload with `author_id` and `author_agent_name`.
- `index_knowledge` (existing, no author) — deleted; every call site
  must move to `_with_author`.
- `reattribute_system_owned(repair_mode: RepairMode) -> RepairReport`
  (NEW) — see contract `contracts/reattribution-cli.md`.

## 5. Qdrant payload schema

**Collection**: `knowledge_items` (production), plus per-test
ephemeral collections.

**Added payload keys**:

| Key | Type | Notes |
|-----|------|-------|
| `author_id` | string | lowercase-hyphenated UUID |
| `author_agent_name` | string | denormalized for quick filter |

Existing keys (project, kind, body, source, tags, created_at,
updated_at) are unchanged. No payload index changes required for
this sprint — filtering by author isn't a hot path yet.

## 6. Re-attribution working state

**Location**: `crates/klams-store/src/repair.rs` (new module).

A `LOST_AUTHOR_ID` constant is added alongside `SYSTEM_AUTHOR_ID` in
[crates/klams-types/src/lib.rs](crates/klams-types/src/lib.rs):

```rust
pub const LOST_AUTHOR_ID: Uuid =
    uuid!("00000000-0000-7000-8000-000000000002");
```

A migration seeds the corresponding row in `authors`
(`agent_name = "lost-author"`) so per-author surfaces can list and
filter it like any other author.

The repair function uses ephemeral in-memory state only (no new
tables). The CLI emits a JSON report and optionally writes it to a
file path passed via `--report-out`.

```rust
pub enum RepairMode {
    DryRun,
    Apply,
}

pub struct RepairReport {
    pub started_at: DateTime<Utc>,
    pub mode: RepairMode,
    pub facts: TableRepairOutcome,
    pub events: TableRepairOutcome,
    pub knowledge_items: TableRepairOutcome,
}

pub struct TableRepairOutcome {
    pub total_system_attributed: u64,
    pub reassigned_to_recovered_author: u64,
    pub reassigned_to_lost_author: u64,    // no provenance, ambiguous, or author deleted
    pub left_as_system: u64,               // genuinely system writes
    pub per_author: Vec<(Uuid, String, u64)>, // (author_id, agent_name, count); includes lost-author bucket
}
```

Invariant: `total_system_attributed ==
reassigned_to_recovered_author + reassigned_to_lost_author +
left_as_system` for every table.

## 7. Connection limit config

**Location**: `crates/klams-service/src/config.rs`

New TOML section:

```toml
[service.limits]
header_read_timeout_secs = 30
keep_alive_timeout_secs = 75
per_peer_max_concurrent = 64
```

Defaults applied when section is absent; documented in
`contracts/connection-limits.md`.
