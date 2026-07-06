# Feature Specification: Advanced Retrieval and Summarization

**Feature Branch**: `005-advanced-retrieval`  
**Created**: 2026-05-20  
**Status**: Draft  
**Input**: User description: "Phase 4 of sprints/planning/plan.md — advanced retrieval and summarization. Hybrid retrieval (vector + Postgres FTS + metadata filters reusing the existing FTS and JSONB GIN indexes from sprint 003), config-driven per-type temporal weighting (currently λ is hard-coded), background summarization for long event logs and stale knowledge clusters, a new POST /memory/context endpoint that returns deduped, summarized structured facts + knowledge + recent events under a token budget, and a viewport context-preview UI. Exit criterion: /memory/context returns a coherent bundle under a configurable token budget for a representative query."

This sprint operationalizes Phase 4 of [the master plan](../planning/plan.md):
"Advanced retrieval and summarization." After sprint 003, klams has live
data flowing in from Ansible plays, the repo/notes scanner, and service
monitors, plus the proposal/dissent machinery from sprint 002. The shape
of the read side is now the bottleneck: `/memory/search` returns one
flat ranked list of vector hits, decay parameters are baked into Rust
constants, and there is no first-class "give me everything an agent
needs to answer this query" endpoint. Phase 4 closes those gaps so an
external agent — or the viewport's preview pane — can ask one question
and get back a coherent, deduped, budget-respecting context bundle
made of structured facts, relevant knowledge chunks, and recent
events.

The sprint deliberately reuses the indexes sprint 003 added (the
`payload::text` `tsvector` FTS index and the JSONB `jsonb_path_ops` GIN
index on `facts`) and the embedding/Qdrant pipeline from sprint 001;
no new data stores are introduced.

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Agents fetch a coherent context bundle for a query (Priority: P1)

An agent (today: the controller and the viewport's preview pane;
soon: the MCP server) needs to answer a question like *"what do we
know about kubs0's GPU configuration and recent service changes?"*
Today the agent has to call `/memory/search` once per memory type,
manually merge results, hand-trim to whatever fits the model context,
and accept whatever duplication shows up across types. Phase 4 ships
`POST /memory/context` so the agent makes one call, supplies a query
string and a token budget, and gets back a structured bundle —
`facts[]`, `knowledge[]`, `events[]` — that is deduped across sections,
ranked by the decay-aware score, and truncated to fit the budget.

**Why this priority**: This endpoint is the entire point of Phase 4 and
the exit criterion the master plan calls out by name. Every other
deliverable in this sprint exists to make this one work. P1.

**Independent Test**: With a representative query (e.g.
`"kubs0 GPU and CUDA toolkit"`), `POST /memory/context` with a token
budget of 4 000 returns a JSON bundle whose total estimated token
count is ≤ 4 000, contains at least one fact, one knowledge chunk,
and zero or more events from the last N days, and contains no item
that appears in two sections. Re-running with a budget of 1 000
returns a strictly smaller bundle drawn from the same ranked set
(no item appears at budget 1 000 that did not appear at budget
4 000).

**Acceptance Scenarios**:

1. **Given** facts, knowledge chunks, and events that all match a
   query, **When** an agent calls `POST /memory/context` with a token
   budget, **Then** the response contains all three sections, the
   sum of estimated tokens across sections is ≤ the requested budget,
   and items are ordered by the decay-aware score within each
   section.
2. **Given** a knowledge chunk whose source file is also referenced
   by a fact (same `file` / `repo` metadata), **When** the bundle is
   built, **Then** the duplicate appears in only one section
   (knowledge wins for prose, facts win for structured attributes —
   the rule is fixed and documented, not query-dependent).
3. **Given** a token budget smaller than the highest-ranked single
   item, **When** the bundle is built, **Then** the response includes
   that one item (truncated or summarized — see User Story 3) with
   `truncated: true` set and never returns an empty bundle for a
   query that produced any matches.
4. **Given** an unhealthy Qdrant or Postgres dependency, **When**
   `/memory/context` is called, **Then** the response indicates which
   memory types were unavailable rather than failing the whole call,
   so an agent operating with degraded retrieval still gets the
   sections that did succeed.

---

### User Story 2 — Hybrid retrieval finds matches that pure vector search misses (Priority: P1)

When Ken (or an agent) searches for an exact identifier — a hostname
like `kubs0`, a service name like `qdrant`, a file path, a CLI flag,
a SHA — pure semantic search routinely ranks paraphrases above the
literal match. Phase 4 makes every read path (`/memory/search` and
the new `/memory/context`) combine vector similarity, Postgres
full-text search over fact payloads and knowledge chunk text, and
metadata filters (host, type, tag, repo, file, source, time window)
into a single ranked result. The fusion is server-side — clients
keep calling the same endpoint with the same query string and get
better ranking automatically.

**Why this priority**: Without hybrid retrieval, the context endpoint
ships with the same recall problems as `/memory/search`, and the
"coherent bundle for a representative query" exit criterion will fail
on any query where the vocabulary the agent uses doesn't match the
vocabulary Ken or Ansible used when writing. P1.

**Independent Test**: Index a fact whose payload contains the literal
string `cuda_toolkit_version=12.4` and a knowledge note that talks
about "the NVIDIA toolkit on the homelab GPU box" without using the
words `cuda` or `12.4`. A search for `cuda 12.4` ranks the fact
first; a search for `nvidia toolkit homelab` ranks the note first.
Both queries previously returned only one of the two; both now
return both, with the literal-match query putting the FTS hit on
top and the paraphrase query putting the vector hit on top.

**Acceptance Scenarios**:

1. **Given** a query that has both a strong literal match in fact
   payloads and a strong semantic match in knowledge chunks,
   **When** the unified ranking runs, **Then** both items appear in
   the top results, with the literal match ranked above the
   paraphrase.
2. **Given** a query with a metadata filter (`host=kubs0`,
   `type=EnvFact`, `since=7d`), **When** the unified ranking runs,
   **Then** every returned item satisfies every filter and the
   filter is applied before the score-based truncation (so a
   tighter filter never produces an emptier result than the
   filter-only path would have).
3. **Given** the FTS and JSONB GIN indexes added in sprint 003,
   **When** the hybrid query plan runs on a representative-size
   facts table (≥ 10 000 rows), **Then** the keyword and filter
   stages use those indexes (verified via `EXPLAIN ANALYZE`) and
   the hybrid query's p95 latency is no worse than 2× the existing
   vector-only path on the same query set.

---

### User Story 3 — Background summarization keeps long event logs and stale knowledge usable (Priority: P2)

Some content stops being individually interesting long before it
stops being collectively interesting. A week of `service.up` /
`service.down` events on a stable host is noise individually but a
useful "host has been stable for 7 days" summary collectively.
Knowledge chunks that haven't been read or updated in months are
candidates for compaction into a digest entry per cluster. Phase 4
adds a background task that produces summary records for both cases
and surfaces them through the same retrieval path as their source
material, so the context endpoint can substitute a summary when
the raw items would blow the token budget.

**Why this priority**: Summarization is what lets the context bundle
*shrink without losing meaning*. Without it, large token budgets
work and small ones produce empty or one-item bundles. The exit
criterion is achievable without it for some queries (User Story 1
acceptance scenario 1) but fails as soon as the matching set grows
beyond the budget. P2 because the system is still usable with raw
truncation as a fallback in the worst case.

**Independent Test**: Insert 200 `service.up` events for `qdrant` on
`kubs0` over a synthetic 14-day window. Run the summarization task.
Within one task cycle, an event-summary record exists covering that
window with the source event ids referenced. A `/memory/context`
query whose ranking includes those events at a budget too small to
fit them all returns the summary instead, with a count and the
summary's source-id list, and never silently drops events without
representation.

**Acceptance Scenarios**:

1. **Given** an event-log cluster of N raw events that all match a
   query, **When** the bundle's allotted token budget for the
   `events` section is smaller than the cost of N raw events,
   **Then** the response substitutes the summary record (with
   `kind: "summary"`, `source_count: N`, and the source ids) and
   spends fewer tokens than the raw items would have.
2. **Given** a knowledge cluster that has not been read or updated
   in longer than the configured stale-threshold, **When** the
   summarization task runs, **Then** a digest knowledge item is
   produced and indexed (vector + metadata) with a back-reference to
   the cluster's source items; the source items remain present and
   retrievable when explicitly asked for.
3. **Given** the summarization task is disabled or has not yet run,
   **When** `/memory/context` is called, **Then** the bundle still
   builds — the endpoint degrades to raw items + truncation rather
   than failing. Summarization is a quality improvement, not a hard
   dependency.

---

### User Story 4 — Decay parameters move from code to config (Priority: P2)

The decay-aware score `relevance × 1/(1+λ·age) × log(1+use_count) ×
confidence` was wired up in Phase 2 with `λ` baked in per memory
type as a Rust constant. Tuning it currently means a code change,
a rebuild, and a restart. Phase 4 lifts λ (and the related decay
controls — half-life, minimum floor, per-type overrides) into the
existing klams config file so Ken can retune retrieval without
shipping a new binary. The values are read at service start and
on `SIGHUP`-style reload (or at minimum on restart, if reload is
out of scope for this sprint).

**Why this priority**: Tuning is the day-after work that makes the
context endpoint actually feel right. The endpoint ships even
without runtime tuning — the current hard-coded λ values are usable
defaults — but operators reasonably expect to retune memory
behavior without a rebuild once real traffic is on the system. P2.

**Independent Test**: Set per-type λ values in `klams.example.toml`
that obviously differ from the current Rust constants (e.g.
`UserFact = 0.0`, `TaskFact = 1.0` per day). Restart the service.
A query that ranked an old `TaskFact` above a fresh `UserFact`
under the old constants ranks them in the opposite order under
the new config, with no code change.

**Acceptance Scenarios**:

1. **Given** a klams config file with per-type decay parameters set,
   **When** the service starts, **Then** scoring uses those values
   and a startup log line records the effective per-type table.
2. **Given** a config file with an invalid value (negative λ,
   non-finite, missing required type), **When** the service
   starts, **Then** it refuses to start with an actionable error
   message naming the bad key — never silently falls back to a
   default that masks the misconfiguration.
3. **Given** a config file that omits decay parameters entirely,
   **When** the service starts, **Then** the current sprint-002
   defaults are used and a log line states that defaults are in
   effect (so a fresh deployment doesn't require a config edit
   just to start).

---

### User Story 5 — Viewport previews what an agent will see for a query (Priority: P2)

The memory viewport already renders facts, search hits, and
proposals. Phase 4 adds a "context preview" pane: a query box, a
token-budget slider, and a rendered view of the
`/memory/context` response broken out by section, with per-section
token counts and a side-by-side raw-vs-summarized toggle. This is
how Ken (and reviewers) eyeball-validate that a representative
query returns a coherent bundle — i.e. how the exit criterion gets
demonstrated.

**Why this priority**: Without the preview, the only way to inspect
`/memory/context` output is `curl | jq`, which is fine for
unit-testing but a bad fit for the qualitative "is this bundle
coherent?" judgement that the exit criterion calls for. P2 because
the backend endpoint is independently usable and testable, but the
sprint's *demonstrable* completion runs through the viewport.

**Independent Test**: Open the viewport, type a representative query
into the context-preview pane, slide the token-budget slider from
its max down to a tight value, and observe the bundle re-render at
each setting with section token counts updating; toggle
raw-vs-summarized and observe the events section switch between
listing raw events and showing the summary record.

**Acceptance Scenarios**:

1. **Given** the viewport is connected to a live klams service and
   a query is entered, **When** the user adjusts the token-budget
   slider, **Then** the viewport calls `POST /memory/context` with
   the new budget and re-renders the bundle within a perceived-
   instant interaction (no full page reload, no flicker between
   sections).
2. **Given** a bundle that contains a summary item, **When** the
   user toggles raw-vs-summarized, **Then** the viewport replaces
   the summary with a fetch of its source items (or the reverse)
   without losing the user's query or budget setting.
3. **Given** klams is unreachable, **When** the user submits a
   query, **Then** the pane shows a clear error, not a spinner,
   and the rest of the viewport's panes (facts, search,
   proposals) remain functional.

---

### Edge Cases

- A query that produces zero matches in every memory type returns
  an empty bundle (not a 404), with the same shape as a populated
  bundle, so clients can render "no results" without a special
  case.
- A token budget of 0 returns the bundle envelope (sections, total
  budget, total spent = 0) with empty `items` arrays — useful for
  "is there anything to know?" probes.
- A token budget exceeding what the matching set could ever fill
  returns the full ranked match set, not padding; `truncated:
  false` and `total_spent < budget`.
- A summary record that has been invalidated (its source items
  changed since the summary was produced) is filtered out of the
  bundle and re-summarization is scheduled; the bundle falls back
  to raw items for that section in the meantime.
- Filters that mention an unknown column or fact type return a
  4xx with the offending key named, never a 500 or a silent empty
  result.
- The hybrid ranker must handle the case where one of {vector,
  FTS, metadata} returns zero rows for the query: the others
  still produce a ranked list rather than the whole call returning
  empty.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST expose `POST /memory/context` accepting at
  minimum a `query` string, a `token_budget` integer, optional
  per-type filters (host, type, tag, repo, file, source,
  since/until), and optional per-section budget overrides; and
  return a structured bundle with `facts[]`, `knowledge[]`,
  `events[]`, plus `total_spent`, `truncated`, and per-section
  metadata (count, spent, source: `raw` or `summary`).
- **FR-002**: System MUST estimate token cost per item using
  `tiktoken cl100k_base` as the canonical cost function (matching
  the OpenAI/Anthropic chat tokenizers MCP clients use), with a
  `chars / 4` fallback estimator that is used when the tiktoken
  encoder fails to load or is explicitly disabled in config. The
  active mode MUST be reported in `/healthz` and in the bundle
  envelope so callers can interpret budget accounting.
- **FR-003**: System MUST select items per section under the budget
  using the existing decay-aware score, with budget allocation
  across sections following a documented rule that never starves
  any section that has a matching item (e.g. minimum floor per
  section before greedy fill, not pure global greedy).
- **FR-004**: System MUST deduplicate across sections using a fixed
  precedence rule (facts beat knowledge for structured attributes,
  knowledge beats events for prose, events beat both for raw
  timeline) so the same underlying datum never appears twice in
  one bundle.
- **FR-005**: System MUST implement hybrid retrieval combining vector
  similarity (Qdrant), Postgres full-text search over fact payloads
  and knowledge chunk text, and metadata filters, fused into one
  ranked list per memory type. The default fusion strategy is
  Reciprocal Rank Fusion (RRF) with a documented `k` parameter;
  weighted score blending (per-source weights, with a documented
  normalization rule) is available as an optional alternative
  selectable in config. Cross-encoder reranking is explicitly out
  of scope for this sprint.
- **FR-006**: System MUST reuse the FTS and JSONB GIN indexes
  introduced in sprint 003 for the keyword and metadata stages of
  hybrid retrieval; no new indexes are introduced unless required
  by `EXPLAIN ANALYZE` performance gates.
- **FR-007**: System MUST move per-type decay parameters (λ,
  optional minimum floor, optional half-life) from Rust constants
  to the existing klams config file (`deploy/config/klams.toml`),
  with sprint-002 defaults retained when the config omits them,
  and refuse to start on invalid values.
- **FR-008**: System MUST run a background summarization task that
  (a) produces event-log summary records for clusters of related
  events that exceed configurable size or age thresholds and (b)
  produces digest knowledge items for clusters of stale knowledge
  chunks, where "stale" is defined by configurable
  `last_used_at` and `updated_at` thresholds.
- **FR-009**: System MUST ensure summary records carry references to
  their source item ids, are filtered out and re-scheduled when
  their sources change, and are surfaced through the same retrieval
  path as their source material (so `/memory/context` can pick
  either depending on budget).
- **FR-010**: System MUST produce summary content via a hybrid
  pipeline: extractive/rule-based summarization runs first (top-K
  event counts and time-bracket headlines for events; representative-
  chunk excerpting for knowledge clusters) and is the canonical
  output when it produces an acceptable digest. The extractive
  output is rejected — and the task falls back to a local LLM call
  against Phi-3-medium served by Ollama on `kubs0`'s GPU — when
  either of these documented thresholds is hit: (a) the summary's
  token cost exceeds 60% of the raw cluster's token cost (i.e. the
  "summary" is not meaningfully smaller); or (b) the summary covers
  less than 50% of the cluster's distinct vocabulary. The LLM
  fallback MUST be optional at the config level (so the service
  still runs when Ollama is down) and the chosen mechanism per
  record MUST be recorded on the summary (`mechanism: "extractive"
  | "llm"`).
- **FR-011**: System MUST keep `/memory/context` resilient to a
  single-store outage: if Qdrant or Postgres is unavailable, the
  endpoint returns the sections that succeeded with a per-section
  status, rather than failing the whole call (mirrors the
  per-section degradation already common in the search path).
- **FR-012**: System MUST extend `/memory/search` to use the same
  hybrid retrieval as `/memory/context` (so existing clients see
  the recall improvement automatically); the response shape is
  unchanged.
- **FR-013**: Viewport MUST add a context-preview pane with a query
  input, a token-budget slider, per-section token-count readouts,
  and a raw-vs-summarized toggle, calling the new endpoint
  directly without going through the existing search pane.
- **FR-014**: System MUST emit Prometheus metrics for the new
  surface: `/memory/context` latency histogram, per-section item
  counts, hybrid retrieval per-source contribution counters,
  summarization task run counters and lag, and decay-config
  reload events.
- **FR-015**: System MUST update `docs/architecture.md`,
  `docs/usage.md`, and the configuration example
  (`deploy/config/klams.example.toml`) to reflect the new endpoint,
  the hybrid ranker, the configurable decay parameters, and the
  summarization task — per the constitution's per-phase docs gate.

### Key Entities *(include if feature involves data)*

- **ContextBundle**: the response shape of `/memory/context`. Holds
  three section arrays (`facts`, `knowledge`, `events`), a global
  `total_spent` token count, a `truncated` flag, and per-section
  metadata (count, tokens spent, source = `raw`/`summary`/`mixed`,
  store status). Not persisted; computed per-request from facts,
  knowledge items, events, and summary records.
- **EventSummary**: a derived record produced by the summarization
  task covering a cluster of related events over a time window.
  Carries the cluster definition (host, category, time range), the
  source event ids, an aggregate count, and a short text summary.
  Stored alongside events; surfaced through the same retrieval
  path; invalidated when its source set changes.
- **KnowledgeDigest**: a derived knowledge item produced by the
  summarization task covering a cluster of stale knowledge chunks.
  Carries the cluster definition (tag, repo, file-path prefix,
  staleness window), source chunk ids, an embedding of the digest
  text, and the digest text. Indexed in Qdrant with metadata
  marking it as a digest; ranked the same as any other knowledge
  item.
- **DecayConfig**: per-memory-type decay parameters loaded from the
  klams config file at start. Holds λ and optional half-life and
  floor per type. Validated at load; never silently defaulted on
  invalid input.
- **HybridQueryPlan**: the per-request planner artifact (in-memory,
  not persisted) that resolves which retrieval sources to consult,
  how to fuse their results, and how to allocate token budget
  across sections. Useful as a structural anchor for testing and
  for the future "explain this ranking" affordance the viewport
  may grow.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For a representative query against a populated klams
  (≥ 1 000 facts, ≥ 5 000 knowledge chunks, ≥ 10 000 events),
  `POST /memory/context` returns a bundle whose total estimated
  tokens are within 10% of the requested budget (over or under) for
  budgets in the range 500–8 000.
- **SC-002**: For a query with a known literal match (a hostname,
  service name, or unique identifier) and a known paraphrase
  match, the literal match appears in the bundle's first three
  items and the paraphrase appears at all — neither is silently
  dropped by the ranker. (This is the recall-and-precision
  acceptance condition of hybrid retrieval.)
- **SC-003**: `/memory/context` p95 latency at budget = 4 000 stays
  within 2× the existing `/memory/search` p95 on the same data
  set, despite touching three retrieval paths plus dedupe plus
  budget-fitting.
- **SC-004**: The summarization task can produce an event summary
  for a 1 000-event cluster and a knowledge digest for a
  100-chunk cluster within one task cycle on `kubs0`'s default
  configuration; cycles do not lap (a new run does not start
  until the previous one finished).
- **SC-005**: Changing a decay λ in the config file and restarting
  the service measurably changes the ordering of a query whose
  top results span memory types of different ages — a tuning
  knob that does nothing is not a tuning knob.
- **SC-006**: In the viewport context-preview pane, sliding the
  token-budget slider triggers an end-to-end re-render of the
  bundle within a perceived-instant interaction window on the
  homelab LAN; no item that was already in the bundle ever
  flickers out and back in for the same query and a monotonically
  decreasing budget.
- **SC-007**: Disabling the summarization task (config flag) and
  re-running the SC-001 measurement still produces a coherent
  bundle within budget — quality may drop, but the endpoint does
  not fail. Summarization is additive, not load-bearing.

## Assumptions

- The Qdrant collection, Postgres `facts`/`events` tables, and the
  FTS + JSONB GIN indexes from sprint 003 are present and healthy
  on the target deployment; this sprint adds no new persistent
  stores.
- The decay-aware score formula and its inputs (`decay_weight`,
  `last_used_at`, `use_count`, `confidence`) are already populated
  by the Phase 2 background task; this sprint reads them, tunes
  them via config, and adds new producers (summary records) but
  does not redesign them.
- The token-budget heuristic chosen during clarification is "good
  enough" for a controller/MCP use case; klams does not need to
  match an external model's tokenizer byte-for-byte to be useful
  on the homelab LAN.
- The summarization mechanism chosen during clarification produces
  text that a downstream agent will accept as substitution for the
  raw items it stands in for; if the LLM-backed option is taken,
  the model and prompt template are part of the implementation
  plan, not of this spec.
- The viewport context-preview pane uses the same auth and
  base-URL configuration as the existing viewport panes; no new
  auth surface is introduced by this sprint.
- Per-section dedupe rules (facts vs knowledge vs events
  precedence) are fine to fix in this sprint; query-dependent
  dedupe is explicitly not in scope and would be a future
  enhancement.
- The dedupe/decay-weight backlog item in
  `sprints/planning/backlog.md` (Phase-7-ish) is *not* pulled in by
  this sprint; this sprint reuses the existing `decay_weight`
  signal as-is.
- "Coherent" in the exit criterion is judged by Ken via the
  viewport preview against a small set of representative queries
  documented in this sprint's `quickstart.md`; there is no
  automated coherence metric.
