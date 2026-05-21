# Phase 1 Data Model: Advanced Retrieval and Summarization

This document defines the entities introduced or modified by sprint 005.
All entities are derived from the spec's "Key Entities" section and the
Phase 0 research decisions.

## 1. ContextBundle (response envelope, not persisted)

Returned by `POST /memory/context`. Computed per-request.

```rust
struct ContextBundle {
    facts: Vec<ContextItem>,        // ranked, deduped, budgeted
    knowledge: Vec<ContextItem>,    // ranked, deduped, budgeted
    events: Vec<ContextItem>,       // ranked, deduped, budgeted
    total_spent: u32,               // tokens spent across sections
    truncated: bool,                // true iff a matching item was dropped for budget
    token_encoder: TokenEncoderId,  // "cl100k_base" | "chars_div4"
    sections: BTreeMap<SectionName, SectionMeta>,
}

struct ContextItem {
    kind: ItemKind,                 // "raw" | "summary" | "digest"
    id: Uuid,                       // fact id, event id, qdrant point id, summary id
    score: f32,                     // post-fusion score
    tokens: u32,                    // estimated tokens for this item
    payload: serde_json::Value,     // the item's content (fact, event, knowledge chunk, summary)
    source_ids: Option<Vec<Uuid>>,  // present iff kind != "raw"
}

struct SectionMeta {
    count: u32,
    tokens_spent: u32,
    source: SectionSource,          // "raw" | "summary" | "mixed"
    status: SectionStatus,          // "ok" | "degraded" | "unavailable"
    degraded_reason: Option<String>,
}
```

Validation rules:
- `total_spent <= request.token_budget` (FR-003).
- An item never appears in more than one section (FR-004).
- A section with `status = "unavailable"` has empty `items` and a
  populated `degraded_reason` (FR-011).

## 2. ContextRequest (request envelope)

```rust
struct ContextRequest {
    query: String,                  // required, non-empty
    token_budget: u32,              // required, may be 0
    filters: Option<RetrievalFilters>,
}

struct RetrievalFilters {
    host: Option<String>,
    type_: Option<FactType>,        // serialized as "type"
    tag: Option<String>,
    repo: Option<String>,
    file: Option<String>,
    source: Option<Source>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}
```

Validation:
- `query.trim()` MUST be non-empty (4xx with `query_required`).
- Unknown filter keys → 4xx with the offending key named (edge case
  bullet from spec).

## 3. EventSummary (Postgres `summaries` table)

```sql
CREATE TABLE summaries (
    id              UUID PRIMARY KEY,
    kind            TEXT NOT NULL CHECK (kind IN ('event')),  -- room for future kinds
    host            TEXT NOT NULL,
    category        TEXT NOT NULL,            -- e.g. "service.*"
    day_bucket      DATE NOT NULL,            -- UTC day
    source_count    INTEGER NOT NULL,
    source_ids      UUID[] NOT NULL,
    summary_text    TEXT NOT NULL,
    mechanism       TEXT NOT NULL CHECK (mechanism IN ('extractive', 'llm')),
    generated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    invalidated_at  TIMESTAMPTZ,
    UNIQUE (kind, host, category, day_bucket)
);

CREATE INDEX summaries_day_bucket_idx ON summaries (day_bucket DESC);
CREATE INDEX summaries_category_idx ON summaries (category);
CREATE INDEX summaries_invalidated_idx ON summaries (invalidated_at)
    WHERE invalidated_at IS NULL;
```

Lifecycle:
- Created by the summarization task when an eligible cluster is
  detected (≥ 50 events for `(host, category, day_bucket)`).
- Invalidated (`invalidated_at = now()`) when any source event is
  deleted or its payload changes; the next task cycle re-derives.
- Surfaced through the same retrieval path as raw events; the context
  builder picks raw vs summary based on budget headroom.

## 4. KnowledgeDigest (Qdrant payload, no new collection)

Stored in the existing `knowledge_items` Qdrant collection with these
payload fields:

```json
{
  "kind": "digest",
  "text": "<summary text>",
  "mechanism": "extractive" | "llm",
  "source_ids": ["<chunk uuid>", ...],
  "source_count": 42,
  "cluster": { "repo": "klams", "file_prefix": "specs/" },
  "generated_at": "2026-05-21T04:03:12Z",
  "invalidated_at": null,
  ...standard knowledge fields (vector, tags, etc.)
}
```

Lifecycle:
- Created by the summarization task when ≥ 20 stale chunks share a
  cluster definition.
- Invalidated by setting `invalidated_at`; the retrieval path filters
  `invalidated_at = null` for digests.
- Embedded with the same TEI model as source chunks so it ranks
  natively.

## 5. DecayConfig (existing — validation added)

Already defined in `crates/klams-types/src/decay.rs`. Sprint 005:
- Adds `DecayConfig::validate(&self) -> Result<(), DecayConfigError>`.
- Each `lambda[type]` MUST be finite and `>= 0.0`.
- Each map key MUST parse as a known `FactType` (rejects typos).
- The service calls `validate()` at startup and refuses to start on
  `Err`.
- On success, emits one `INFO` line of the form
  `decay config loaded: UserFact=1e-9 TaskFact=1e-6 EnvFact=1e-9
  interval=3600s batch=500`.

## 6. HybridQueryPlan (in-memory, not persisted)

```rust
struct HybridQueryPlan {
    query: String,
    filters: RetrievalFilters,
    fusion: FusionStrategy,         // RRF { k } | Weighted { weights, normalization }
    per_source_top_k: u32,          // each source returns top-k before fusion
    sources: Vec<RetrievalSource>,  // Vector | Fts | MetadataOnly
}
```

Validation: `per_source_top_k <= 200` (cap to bound work per request).

## 7. SectionSource / ItemKind / TokenEncoderId / FusionStrategy

```rust
enum SectionSource { Raw, Summary, Mixed }
enum ItemKind { Raw, Summary, Digest }
enum TokenEncoderId { Cl100kBase, CharsDiv4 }
enum FusionStrategy {
    Rrf { k: u32 },                                 // default k = 60
    Weighted { vector: f32, fts: f32, normalization: WeightedNorm },
}
enum WeightedNorm { ZScore, MinMax }
```

## 8. New configuration blocks (`klams.toml`)

```toml
[retrieval]
fusion = "rrf"           # "rrf" | "weighted"
rrf_k = 60               # used when fusion = "rrf"
per_source_top_k = 100

# Optional, used only when fusion = "weighted":
# [retrieval.weights]
# vector = 1.0
# fts    = 1.0

[tokens]
mode = "tiktoken"        # "tiktoken" | "fallback"
# fallback is chars/4. Use fallback in tests / when tiktoken data is unavailable.

[summarization]
enabled = true
event_cluster_min = 50
knowledge_stale_days = 90
knowledge_cluster_min = 20
llm_fallback = true
ollama_url = "http://127.0.0.1:11434"
ollama_model = "phi3:medium"
task_interval_seconds = 3600

# Existing block, no changes:
# [decay]
# task_interval_seconds = 3600
# batch_size = 500
# [decay.lambda]
# UserFact = 1e-9
# TaskFact = 1e-6
# EnvFact  = 1e-9
```

## 9. New error variants

```rust
enum ConfigError {
    DecayLambdaNegative { type_: String, value: f32 },
    DecayLambdaNonFinite { type_: String },
    DecayUnknownType { type_: String },
    RetrievalFusionUnknown { value: String },
    SummarizationOllamaUrlInvalid { value: String, source: url::ParseError },
}
```

All are reported with the offending key name; the service exits
non-zero on any.
