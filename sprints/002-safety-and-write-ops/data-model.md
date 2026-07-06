# Data Model: Safety, Drift Control, and the User View

This document describes the **delta** over the 001 MVP data model
([001-initial-mvp/data-model.md](../001-initial-mvp/data-model.md)).
Everything in 001's data model carries forward unchanged unless this
document overrides it. SQL DDL is illustrative; the source of truth
is `migrations/0002_dissents.sql`.

## Entity changes

### Fact (Postgres `facts`) — delta

One new column. No existing columns change semantics.

| Field | Type | Notes |
|---|---|---|
| `dissent_count` | INT NOT NULL DEFAULT 0 | Cached count of pending dissents for this fact. Maintained by triggers on `dissents` (and on `facts` for cascade-orphaning). Surfaced on every `Fact` read and on every `SearchHit` whose `type = fact`. |

No new indexes on `facts`. The existing optimistic-concurrency contract
on `facts.version` is now **enforced** at write time per FR-008 (in
001 the column existed but was not contested by parallel writes).

### Dissent (Postgres `dissents`) — new

A pending proposal from a lower-trust source that contradicts a
higher-trust canonical fact, or a resolved record of a past proposal.

| Field | Type | Notes |
|---|---|---|
| `id` | UUID PK | Generated server-side (v7). |
| `fact_id` | UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE | The canonical fact this proposal targets. |
| `proposed_payload` | JSONB NOT NULL | The contradicting payload the lower-trust writer submitted. |
| `payload_hash` | BYTEA NOT NULL | SHA-256 of canonical JSON over `proposed_payload`. Used for FR-013 dedupe. |
| `source` | TEXT NOT NULL | The original proposing source (e.g. `AgentProposal`). Must be strictly lower trust than `facts.source` at submission time. |
| `submitted_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | First time this proposal was seen. |
| `last_seen_at` | TIMESTAMPTZ NOT NULL DEFAULT now() | Updated when a duplicate submission is deduped. |
| `submission_count` | INT NOT NULL DEFAULT 1 | Incremented when a duplicate is deduped. |
| `status` | TEXT NOT NULL DEFAULT 'pending' | CHECK IN (`'pending'`, `'promoted'`, `'discarded'`, `'orphaned'`). |
| `resolved_at` | TIMESTAMPTZ NULL | Set when `status` leaves `pending`. |
| `resolved_by_source` | TEXT NULL | The trusted source (`User` or `Controller`) that promoted or discarded. |

Indexes:

```sql
CREATE INDEX dissents_fact_id_idx       ON dissents (fact_id);
CREATE INDEX dissents_status_idx        ON dissents (status);
CREATE INDEX dissents_pending_age_idx   ON dissents (submitted_at) WHERE status = 'pending';
-- FR-013 dedupe: at most one pending proposal per (fact, payload).
CREATE UNIQUE INDEX dissents_pending_dedupe_idx
    ON dissents (fact_id, payload_hash) WHERE status = 'pending';
```

Triggers:

```sql
-- Maintain facts.dissent_count after any dissents row insert/update of status.
CREATE FUNCTION refresh_fact_dissent_count(p_fact_id UUID) RETURNS VOID AS $$
BEGIN
    UPDATE facts
       SET dissent_count = (
           SELECT count(*) FROM dissents
            WHERE fact_id = p_fact_id AND status = 'pending'
       )
     WHERE id = p_fact_id;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER dissents_after_insert
AFTER INSERT ON dissents
FOR EACH ROW EXECUTE FUNCTION refresh_fact_dissent_count_tg();

CREATE TRIGGER dissents_after_status_update
AFTER UPDATE OF status ON dissents
FOR EACH ROW
WHEN (OLD.status IS DISTINCT FROM NEW.status)
EXECUTE FUNCTION refresh_fact_dissent_count_tg();

-- Before a fact is deleted, orphan any pending dissents (the ON DELETE
-- CASCADE would otherwise drop them silently; we want a deterministic,
-- observable resolution per the spec's edge case).
CREATE TRIGGER facts_before_delete_orphan_dissents
BEFORE DELETE ON facts
FOR EACH ROW EXECUTE FUNCTION orphan_pending_dissents_tg();
```

(The `_tg()` wrappers are trivial trampolines around the helper to
fit the `RETURNS trigger` signature; full source lives in
`migrations/0002_dissents.sql`.)

State transitions:

- **Create** (lower-trust contradiction observed):
  `INSERT INTO dissents (…) ON CONFLICT (fact_id, payload_hash) WHERE status='pending'
   DO UPDATE SET submission_count = dissents.submission_count + 1,
                 last_seen_at = now()
   RETURNING id`.
- **Promote** (`User` or `Controller` calls promote endpoint): inside a
  single transaction, (1) check `facts.version` matches the caller's
  expected version (HTTP 409 otherwise), (2) `UPDATE facts SET payload =
  dissent.proposed_payload, payload_hash = dissent.payload_hash, version
  = version + 1, source = caller_source, updated_at = now() WHERE id =
  dissent.fact_id`, (3) `UPDATE dissents SET status='promoted',
  resolved_at=now(), resolved_by_source=caller_source WHERE id=dissent.id`,
  (4) trigger recomputes `dissent_count`.
- **Discard** (`User` or `Controller` calls discard endpoint): `UPDATE
  dissents SET status='discarded', resolved_at=now(),
  resolved_by_source=caller_source WHERE id=dissent.id AND status='pending'`.
- **Orphaned** (canonical fact deleted): `BEFORE DELETE` trigger marks
  every pending dissent for the fact as `orphaned` and sets `resolved_at
  = now()`, `resolved_by_source` left NULL. The CASCADE then physically
  deletes them — `orphaned` is the terminal in-flight observation;
  consumers see it via metrics (`klams_dissents_total{outcome="orphaned"}`)
  and structured logs.

## Validator and sanity-rule shapes (in-memory, no DB)

Validators live in `klams-core::validate`. Two layers:

```rust
// klams-types
pub struct ValidationError {
    pub field: String,        // dotted path, e.g. "payload.hostname"
    pub rule: String,         // machine-readable rule id, e.g. "hostname_shape"
    pub message: String,      // human-readable
    pub value: Option<serde_json::Value>, // offending value when safe to echo
}

pub type ValidationResult = Result<(), Vec<ValidationError>>;

// klams-core
pub trait Validator: Send + Sync {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult;
}

pub struct ValidatorRegistry {
    per_type: HashMap<FactType, Vec<Box<dyn Validator>>>,
    sanity:   Vec<Box<dyn Validator>>, // run for every write
}
```

Sanity rules shipped this sprint:

- `timestamp_range` — any field named like a timestamp (per a small
  allowlist: `at`, `created_at`, `updated_at`, `last_used_at`,
  `*_at`, `*_time`) MUST parse as RFC3339 and fall within ±10 years
  of the service-process wall clock.
- `hostname_shape` — any field named `hostname` / `host` MUST match
  the conservative LDH+dots pattern `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$`,
  case-folded before matching.
- `numeric_range` — fields tagged as numeric in per-type validator
  metadata MUST fall within the per-field declared range.

Per-type validators shipped this sprint (one function per
`FactType`):

- `UserFact`: `required {name}`, `name` is non-empty string ≤ 256
  chars, optional `email` matches a minimal mailbox regex, optional
  `birthdate` parses as RFC3339 date.
- `TaskFact`: `required {task_id, status}`, `task_id` is UUID,
  `status ∈ {planned, in_progress, blocked, done, cancelled}`,
  optional `hostname` triggers `hostname_shape`.
- `EnvFact`: `required {key, value}`, `key` is non-empty string
  ≤ 256 chars matching `^[A-Z][A-Z0-9_]*$`, `value` is string ≤ 4096
  chars, optional `hostname` triggers `hostname_shape`.

Event validators (registered against `category`):

- `Service`: `required {hostname, name, state}`, `state ∈ {up, down,
  degraded}`, `hostname` triggers `hostname_shape`.
- `Execution`: `required {command, exit_code}`, `exit_code` integer
  in `[-128, 255]`, optional `duration_ms` integer ≥ 0.
- Other categories accept any structurally-valid payload (sanity rules
  still apply) per FR-004.

Knowledge: existing per-record limits in 001 (`text` length, `tags`
count) remain authoritative; no new per-record rules this sprint.

## Decay config shape

Loaded from `[decay]` in the service config; per-type λ overrides
defaults, missing types use defaults.

```rust
pub struct DecayConfig {
    pub task_interval: Duration,         // default 3600s
    pub batch_size: u32,                 // default 500
    pub lambda_per_type: HashMap<FactType, f32>,
}

impl DecayConfig {
    pub fn lambda_for(&self, t: FactType) -> f32 {
        self.lambda_per_type.get(&t).copied().unwrap_or_else(|| match t {
            FactType::UserFact => 1e-9,
            FactType::TaskFact => 1e-6,
            FactType::EnvFact  => 1e-9,
        })
    }
}
```

TOML:

```toml
[decay]
task_interval_seconds = 3600
batch_size = 500

[decay.lambda]
UserFact = 1e-9
TaskFact = 1e-6
EnvFact  = 1e-9
```

## Pipeline type — delta

The `MemoryWrite` enum from 001 is unchanged. The **result** of a
fact upsert now carries one of two variants so the API layer can map
to 200 vs 202 vs 409:

```rust
// klams-types
pub enum FactWriteOutcome {
    Persisted { fact: Fact },                                 // HTTP 200
    Dissented { dissent_id: Uuid, fact_id: Uuid },            // HTTP 202
    VersionConflict { current_version: i32, fact_id: Uuid },  // HTTP 409
}
```

The handler's oneshot reply channel from 001 now carries
`Result<FactWriteOutcome, WorkerError>`.

## API error envelope — delta

The existing `ApiError` wire type gains an optional `details` array
and a new error code, while staying backwards-compatible with every
001 consumer (extra fields are ignored by legacy clients).

```rust
// klams-types::ApiError
pub struct ApiError {
    pub code:       String,                       // existing
    pub message:    String,                       // existing
    pub field:      Option<String>,               // existing (single-field shortcut)
    pub request_id: Option<String>,               // existing
    pub details:    Option<Vec<ErrorDetail>>,     // NEW — populated for validation_error
    pub current_version: Option<i32>,             // NEW — populated for version_conflict
}

pub struct ErrorDetail {
    pub field:   String,
    pub rule:    String,
    pub message: String,
    pub value:   Option<serde_json::Value>,
}
```

New `code` values introduced this sprint:

- `version_conflict` (HTTP 409) — set on canonical-write or
  dissent-promote attempts whose expected `version` does not match.
- `trust_required` (HTTP 403) — set when a non-`User`/non-`Controller`
  source attempts to promote or discard a dissent.

## Provenance bundle (viewport)

Pure-frontend type, no DB schema. The viewport assembles this for
each entity displayed in the provenance panel:

```ts
// viewport/src/lib/types.ts
export type ProvenanceBundle = {
    source: Source;
    version: number;
    createdAt: string;
    updatedAt: string;
    lastUsedAt: string | null;
    decayWeight: number;
    confidence: number;
    dissentCount: number;       // 0 for events and knowledge items
};
```

Source values are mapped 1:1 from the API's `Source` enum.
`dissentCount` is omitted (rendered as zero/hidden) for events and
knowledge items where the concept does not apply.
