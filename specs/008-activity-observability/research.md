# Research — Activity & Observability (sprint 008)

This document resolves Technical Context unknowns and records design
decisions referenced from [plan.md](./plan.md). Each entry follows
**Decision / Rationale / Alternatives** so future readers can audit
the trade space.

---

## R-001 — Endpoint surface split: MCP tool vs HTTP endpoint

**Decision**: Ship two distinct surfaces backed by one query layer.

- `event_search` is an **agent-facing MCP tool** (under the existing `/mcp` mount, `read` scope). It returns **only events**, filtered by `author_id` / `category` / `since` / `until` / `payload_match`. It exists because events have no embeddable text and therefore cannot be retrieved by `memory_search`.
- `GET /v1/memories` is an **operator-facing HTTP endpoint** (under the existing protected `/v1/*` mount, `read` scope). It returns the unified `PublicMemory` projection across **all kinds** (`fact`, `knowledge`, `event`) and **all authors**, with the same per-row shape used by `GET /v1/authors/{id}/memories`. It backs the viewport's `/activity` tab and is the general "what's happened recently" API.

Both surfaces dispatch into a single new `Store::list_memories(...)` / `Store::event_search(...)` pair on the store trait. The SQL date-window predicate, cursor encoding, and projection construction are shared.

**Rationale**:

- Consumers differ. Agents talk MCP and care only about events (because they already use `memory_search` for facts/knowledge). Operators want one row per item across all kinds with the same drilldown links the per-author view provides.
- Projections differ. `event_search` returns `PublicMemoryContent::Event` exclusively. `GET /v1/memories` returns the discriminated union and conditionally surfaces `deleted_at` / `deleted_by_author_id`.
- Rate-limit / window-cap profiles differ. The HTTP endpoint is bounded by the FR-009 30-day cap because operator queries scan all kinds. `event_search` shares the same 30-day cap (events are cheap, but applying the cap uniformly avoids special-casing).
- Collapsing into one surface would force one consumer to subset the other's projection or scope set — extra glue with no operator win.

**Alternatives considered**:

- **Single MCP tool that returns all kinds**: rejected — the viewport doesn't speak MCP and we'd lose the REST contract test parity with `/v1/authors/{id}/memories`.
- **Single REST endpoint with `?kinds=event`**: rejected — agent surface would have to learn HTTP auth and request shapes already different from the MCP envelope; defeats sprint 007's "tools for agents, REST for the viewport" split.
- **Three endpoints (events-only HTTP, all-kinds HTTP, events-only MCP)**: rejected — extra surface area for no agent or operator benefit beyond what the two-surface split already gives.

---

## R-002 — Window cap policy

**Decision**: A single global maximum window of **30 days** between `since` and `until`, configured via `Config::api.memories_max_window_days: u32` (default 30). Requests exceeding the cap return `400 WINDOW_TOO_LARGE` with the configured maximum surfaced in the error body. The same cap applies to both `event_search` and `GET /v1/memories` (FR-009 + edge case in spec).

**Rationale**:

- 30 days covers every operator "what's happened lately?" use case in practice without ever scanning the full corpus.
- A single global knob is the minimum-viable policy lever. Per-token or per-author windows are explicitly out of scope (spec Assumptions).
- Sprint 007's `klams-types::ApiConfig` already exists; adding one `u32` field is a one-line, backward-compatible change with `#[serde(default = "default_memories_max_window_days")]`.
- Retrospective queries beyond 30 days are still possible — the operator paginates by window in client code (or a future sprint surfaces a long-window read endpoint with explicit cost controls).

**Alternatives considered**:

- **No cap at all**: rejected — unbounded scans against a growing store eventually take the API offline on cold cache; the cap is cheap insurance.
- **Different caps per surface (e.g. 7 days for `event_search`, 30 days for `/v1/memories`)**: rejected — surface skew with no clear operator story; uniform cap is easier to document.
- **Per-token cap in config**: rejected — adds a config knob that no current consumer needs and that conflicts with R-005 from sprint 007 ("tokens are interchangeable agents of Ken, not multi-tenant").

---

## R-003 — Cursor encoding for cross-kind pagination

**Decision**: Reuse sprint 007's cursor pattern. Cursors are opaque base64-url-safe strings encoding a `section:ts_nanos:id` triple where `section ∈ {"f","e","k"}` discriminates the per-kind sub-query (facts / events / knowledge), `ts_nanos` is the row's `created_at` in nanoseconds, and `id` is the row UUID. This is byte-identical to the `encode_memory_cursor` helper already used by `Store::list_author_memories` in `crates/klams-store/src/composite.rs`.

For `event_search` the cursor degenerates to `e:ts_nanos:id` (only one section).

For `GET /v1/memories` the handler walks the sections in priority order matching the spec's "newest-first" intent: it merges per-kind pages with an in-memory `(created_at, id)` heap-style merge, but it advances the cursor on the **earliest unconsumed row** of the still-paginating section. Concretely: when the handler has buffered N rows from each requested kind, it emits up to `limit` rows from the merged stream and encodes the cursor of the **smallest** kept row's section so the next page resumes from there.

**Rationale**:

- Re-using the existing cursor codec means no new wire format and no client-side migration.
- Cross-kind merge using a per-section cursor (rather than a single global `created_at` cursor) is robust against pages where one kind has many recent items and another has none — without it, the handler would skip rows.
- The "earliest unconsumed row" cursor encoding is the standard pattern for K-way merge pagination and is well-understood operationally.

**Alternatives considered**:

- **Single `(created_at, id)` cursor across all kinds**: rejected — fails when a kind has rows older than the merged page boundary (they would be invisible on subsequent pages).
- **Server-side row materialization in a temporary table**: rejected — keeping cursor state server-side defeats the simplicity of the existing pattern; opaque base64 is cheaper.
- **GraphQL-style "after cursor for kind X" array in the response**: rejected — adds a new response shape; the merged-cursor approach delivers the same effect with the existing shape.

---

## R-004 — Cross-author query layer in `klams-store`

**Decision**: Add two new methods to the `Store` trait, sitting next to `list_author_memories`:

```rust
async fn list_memories(&self, q: ListMemoriesQuery)
    -> StoreResult<(Vec<ListMemoriesRow>, Option<String>)>;

async fn event_search(&self, q: EventSearchQuery)
    -> StoreResult<(Vec<PublicMemory>, Option<String>)>;
```

`ListMemoriesQuery` carries `(since, until, kinds, state, authors, limit, cursor)`. `EventSearchQuery` carries `(author_id, categories, since, until, payload_match, limit, order, cursor)`. Both delegate to per-kind page fetchers on the existing `PostgresStore` (`list_memories_facts_page`, `list_memories_events_page`, `event_search_page`) and `QdrantStore` (`list_memories_knowledge_page`). The composite store performs the cross-kind merge for `list_memories`.

Bulk author lookup uses the existing `bulk_fetch_authors` helper introduced in sprint 007 — the call site is mechanically identical to `list_author_memories_impl`.

**Rationale**:

- Mirrors sprint 007's trait layout exactly, including the `(rows, next_cursor)` return tuple. Reviewers and future maintainers see one pattern, not two.
- Keeping the trait surface narrow (two methods, not six) keeps the test-double surface small.
- The per-kind fetchers are co-located with the storage they wrap (`postgres.rs` for facts/events, `qdrant.rs` for knowledge), so the SQL or scroll-API specifics stay encapsulated.

**Alternatives considered**:

- **Reuse `list_author_memories` with `author_id = None`**: rejected — the existing method's contract requires a single author UUID, and widening the contract would force every existing caller through new error paths.
- **Single `list_memories` method that handles both event-only and cross-kind**: rejected — the event-only path with `payload_match` doesn't fit the cross-kind cursor merge; separate methods keep each query's edge cases independent.
- **Push the merge into Postgres via a SQL view**: rejected — knowledge items live in Qdrant, not Postgres, so the merge must happen in the service layer regardless.

---

## R-005 — Cross-kind ordering and merge semantics

**Decision**: `GET /v1/memories` orders rows by `(created_at DESC, id DESC)` across all kinds. The composite store fetches up to `limit + 1` rows from each requested kind (over the date window, post-cursor), merges in memory, returns the top `limit`, and encodes the cursor on the smallest kept row. Knowledge items come from Qdrant via a payload-filtered scroll keyed on the `created_at` payload field (set by every knowledge write since sprint 005).

For ties on `(created_at, id)` (impossible in practice but explicit in the spec): the merge is stable per-kind, and the kind order `fact > knowledge > event` breaks any tie deterministically.

**Rationale**:

- `(created_at, id)` is the only ordering that makes "what just happened" coherent across kinds, and it is already the existing per-author drilldown ordering — no new operator concept.
- Fetching `limit + 1` per kind keeps the merge cheap (max `3 * (limit + 1)` rows in memory; with `limit ≤ 200` that's ≤ 603 rows). No risk of unbounded memory growth.
- Qdrant scrolls are O(limit) under the payload filter when `created_at` is an indexed payload field — confirmed in sprint 005's hybrid retrieval work.

**Alternatives considered**:

- **`(updated_at, id)` ordering**: rejected — facts can be updated by the decay task, which would shuffle them to the top of the activity view for no operator-meaningful reason.
- **Per-kind sort then concatenate**: rejected — produces a non-chronological list (all facts, then all events, etc.); breaks the user story.
- **Streaming merge with server-sent events**: rejected — premature; pagination handles the size today.

---

## R-006 — Grafana panel failure root cause

**Decision**: Two issues conspire to produce "No Data":

1. **No `klams_mcp_*` panels exist in `deploy/grafana/klams.json` yet.** Confirmed by searching the dashboard JSON: zero matches for `klams_mcp`. Sprint 007 added the counters in `crates/klams-mcp/src/metrics.rs` but did not add the dashboard panels.
2. **No `deploy/prometheus/` directory exists in the repo.** Sprint 007's deployment runbook assumed Prometheus config lived on the operator's host; a clean checkout has nothing to scrape `klams-service` with.

The fix is twofold and lives entirely in `deploy/`:

- Add three "MCP author activity" panels to `deploy/grafana/klams.json` using the PromQL queries enumerated in [contracts/grafana-mcp-panels.md](./contracts/grafana-mcp-panels.md). The label set matches what `crates/klams-mcp/src/metrics.rs` actually emits: `agent_name`, `model` on all three counters; plus `kind` on `klams_mcp_writes_total` and `mode` on `klams_mcp_deletes_total`.
- Add `deploy/prometheus/prometheus.yml` with a scrape job for the systemd-deployed `klams-service` (`scrape_configs.static_configs.targets: ["host.docker.internal:7777"]` or the equivalent depending on whether Prometheus runs in the compose stack or on-host). Document both modes in `deploy/prometheus/README.md`.

No `klams-service` code change is required. R-010 from sprint 007 (label cardinality discipline) is unchanged.

**Rationale**:

- Aligns with the spec's "config drift, not service change" assumption.
- Checking in `deploy/prometheus/` is the minimum-viable way to make FR-018 ("reproducible from a clean checkout") true.
- Three panels (writes, deletes, search) match the three counters one-to-one; no extra Grafana surface invented.

**Alternatives considered**:

- **Add a `klams-mcp` recording rule in Prometheus**: rejected — premature; the raw counters are scraped at low cardinality already.
- **Vendor a Grafana provisioning YAML alongside `klams.json`**: rejected — out of scope for this sprint; the existing manual-import workflow continues to work.
- **Embed Prometheus in the compose stack and remove the on-host mode**: rejected — Ken's existing observability stack runs Prometheus on-host; forcing a compose-side Prometheus would break his existing dashboards.

---

## R-007 — Perf fixture: deterministic seeded generator

**Decision**: Implement the fixture as a Rust binary (`klams-bench` crate, `src/bin/seed.rs`) that:

1. Takes a `--seed u64` CLI flag (default `0xC0FFEE_0008`).
2. Uses `rand_chacha::ChaCha20Rng::seed_from_u64(seed)` for reproducibility — `ChaCha20` is endian- and platform-independent, so the same seed produces the same corpus regardless of dev host.
3. Generates ≥ 10,000 facts (mix of all `FactType` variants, with realistic-shaped payloads sampled from a small template library) and ≥ 50,000 knowledge items (Lorem-ipsum-style text bodies seeded by the same RNG; embeddings computed by the live TEI service the seed binary connects to).
4. Writes facts via the existing `PostgresStore` API and knowledge via the existing `QdrantStore + TEI` pipeline. **No bypass paths** — the seed exercises the same write surface a real agent would.
5. On rerun with the same seed, the existing dedupe pipeline absorbs the writes idempotently (canonical-hash dedupe is already in place for facts; content-hash for knowledge).

The corpus evolves with the schema because it writes via the canonical store traits — any future field addition automatically gets a value (sourced from the template library or defaulted), and the binary fails to compile if a required field disappears.

**Rationale**:

- Captured artifact (e.g. a pg_dump) would drift the moment any schema column moves; a generator is self-updating.
- `ChaCha20Rng` is the standard cryptographically-strong deterministic RNG in the Rust ecosystem; it's faster than `StdRng` and explicitly portable.
- Writing through the real traits guarantees the fixture's perf characteristics match production writes (indexes, triggers, embedding round-trips).
- Default seed `0xC0FFEE_0008` is a one-line operator-visible signal that this fixture is sprint-008 perf scaffolding, not random.

**Alternatives considered**:

- **Pre-generated SQL dump checked in**: rejected — drifts on every schema change; large file in git.
- **Synthetic SQL `INSERT` directly into Postgres bypassing the store**: rejected — bypasses dedupe and embedding cost, producing optimistic (and useless) p95 numbers.
- **Property-based generator (`proptest`)**: rejected — proptest is for shrinking, not for stable corpora; overhead doesn't fit.

---

## R-008 — Benchmark harness and output

**Decision**: Implement the harness as a second binary (`klams-bench`, `src/bin/run.rs`) that:

1. Loads a representative query set from `tools/bench/queries.txt` (10 short prompts spanning facts, knowledge, and mixed). The set is checked in so reruns measure the same shape.
2. For each query, calls `klams-client::memory_search` 10 times, totalling 100 measurements.
3. Records each call's wall-clock latency into an `hdrhistogram::Histogram<u64>` (microsecond resolution, range 1 µs – 60 s).
4. Computes p50, p95, p99, min, max, and mean from the histogram.
5. Writes `specs/008-activity-observability/perf-baseline.md` with the numbers, the timestamp, the seed, the host hostname (for context), and the row counts observed in the store (sanity check the fixture loaded).
6. **Exits 0 regardless of whether p95 < SC-006's 1 s threshold.** Reporting is the deliverable (FR-022).

The output markdown's header is templated so reruns produce a clean diff:

```markdown
# Perf baseline — sprint 008

> Generated 2026-05-25T17:42:11Z by `just bench-run` on `kubs0`.
> Fixture seed: `0xC0FFEE_0008` · Store: 10,247 facts, 50,138 knowledge items.

| Metric         | Value         |
| -------------- | ------------- |
| Samples        | 100           |
| p50 latency    | ...           |
| p95 latency    | ...           |
| p99 latency    | ...           |
| min / max      | ... / ...     |
| mean           | ...           |
```

**Rationale**:

- `hdrhistogram` is the de-facto Rust library for percentile measurement; pulling it in as a `tools/bench`-only dev-dep keeps the production graph clean.
- 100 samples is the minimum that makes a p99 reading meaningful; sprint 007 itself specified 100 calls.
- Checked-in query set + checked-in markdown lets future sprints diff perf numbers without rebuilding the query corpus.

**Alternatives considered**:

- **Use `criterion`**: rejected — criterion is for micro-benchmarks with warmup curves and noise filtering; here we want end-to-end p95 against a real service.
- **Output JSON instead of markdown**: rejected — operator opens the README link expecting prose context, not a JSON dump.
- **Auto-open a PR if p95 regresses**: rejected — FR-022 forbids; reporting is the entire deliverable.

---

## R-009 — Viewport `/activity` UI architecture

**Decision**: Add `/activity` as a top-level SvelteKit route. The page:

1. Loads via a SvelteKit `+page.ts` that calls a new `list_memories` Tauri command.
2. The Tauri command in `viewport/src-tauri/src/commands/memories.rs` proxies to `GET /v1/memories` using the existing `klams-client` crate (which gains a `list_memories` method exactly mirroring the contract).
3. Renders one row per memory using the existing per-author drilldown row component (factored out of `viewport/src/routes/authors/[id]/+page.svelte` into a shared `MemoryRow.svelte` component as part of this sprint).
4. Click-through deep-links navigate to `/facts/:id`, `/knowledge/:id`, `/events/:id` — routes that already exist.
5. Filter controls live in a left sidebar: from/to datetime pickers (default last 24h), kind dropdown, state dropdown, author multi-select (populated by the existing `GET /v1/authors` call).

The author multi-select piggybacks on the `/authors` data loader — no new request shape.

**Rationale**:

- Refactor of `MemoryRow.svelte` is a small (~30-line) extraction that also pays down a duplicate-rendering wart in the existing per-author view; net-positive churn.
- Tauri command proxy pattern is the same one the per-author view already uses; reviewers see one shape.
- Defaults match the spec's US2 acceptance scenario exactly (24h, all kinds, live).

**Alternatives considered**:

- **Replace `/authors` with `/activity` as the landing tab**: rejected — out of scope; sprint 008 is additive.
- **Direct fetch from SvelteKit to klams-service bypassing Tauri**: rejected — breaks the auth model (the bearer token is held in the Tauri side, not the renderer).
- **Render rows server-side (SSR)**: rejected — the viewport is a desktop app; SSR is meaningless here.

---

## R-010 — `payload_match` semantics for `event_search`

**Decision**: `payload_match` is an object of key → value pairs. The server requires **exact equality** on each key when looked up in the event's `payload` JSONB column. Equality is JSONB-level (string vs number distinction matters; `"42"` does not match `42`). Implemented in Postgres via a `payload @> $1::jsonb` containment predicate, with the `payload_match` object cast to JSONB once at query-build time.

Missing keys in the event payload count as non-matches. Nested objects are matched by JSONB containment (so `{"service":"widget"}` matches `{"service":"widget","region":"us-east"}` but not `{"service":"widget-prime"}`). Arrays are matched as JSONB containment of the array (sub-array match).

**Rationale**:

- JSONB containment (`@>`) is the standard Postgres operator, has an opclass-backed GIN index plan, and matches the operator intuition ("filter by these key/value pairs").
- Avoids inventing a DSL for ranges, regex, or `OR` — the spec calls out exact equality only.
- A GIN index on `events.payload` (already present from sprint 003's event schema) keeps the predicate sub-millisecond at corpus size.

**Alternatives considered**:

- **JSON-path expressions**: rejected — overkill, harder to validate at the contract layer.
- **String-equality on serialized payload**: rejected — order-dependent, breaks any client that re-serializes its payload.

---

## R-011 — Error-code additions and their stability

**Decision**: Two new entries appended to the canonical error-code list:

- `WINDOW_TOO_LARGE` — requested `until − since` exceeds `api.memories_max_window_days`. Surfaces the configured maximum in `_meta.window_max_days`. Returned as `400` from HTTP and as the standard MCP error envelope from `event_search`.
- `INVALID_WINDOW` — `since > until`. Returned as `400` from HTTP and as the standard MCP error envelope from `event_search`.

Both are added to [contracts/error-codes.md](./contracts/error-codes.md) in this sprint and mirror sprint 007's stability promise: codes are part of the public contract and renames require a spec amendment.

No existing error codes are renamed or repurposed.

**Rationale**:

- Keeping the two new codes alongside sprint 007's set means clients have one place to look.
- `WINDOW_TOO_LARGE` deserves its own code (not a generic `INVALID_ARGUMENT`) because the remediation is specific: "shrink your window or paginate".
- `INVALID_WINDOW` likewise: the client must fix the order, not retry.

**Alternatives considered**:

- **Generic `INVALID_ARGUMENT` with a descriptive message**: rejected — opaque to programmatic clients; goes against sprint 007's stable-code policy.
- **Reuse `INSUFFICIENT_SCOPE` or similar**: rejected — orthogonal failure mode.
