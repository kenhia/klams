# Bench Harness — CLI surface (sprint 008)

Authoritative CLI shape for the perf fixture and harness in
`tools/bench/`, plus the output-file format for
`sprints/008-activity-observability/perf-baseline.md`.

---

## Package layout

```text
tools/bench/
├── Cargo.toml            # package = "klams-bench"
├── src/
│   ├── lib.rs            # corpus generator + histogram → markdown
│   └── bin/
│       ├── seed.rs       # deterministic seeded fixture generator
│       └── run.rs        # 100-call memory_search harness
├── queries.txt           # 10 representative queries (one per line)
└── README.md             # operator usage notes
```

The package is a workspace member but **not** a dependency of any
production binary.

---

## `seed` binary

**Recipe**: `just bench-seed`

**Invocation**:

```bash
cargo run --release -p klams-bench --bin seed -- \
    --seed 0xC0FFEE_0008 \
    --facts 10000 \
    --knowledge 50000 \
    --klams-url http://localhost:7777 \
    --klams-token "${KLAMS_TOKEN}"
```

**Flags**:

| Flag           | Default          | Notes |
|----------------|------------------|-------|
| `--seed`       | `0xC0FFEE_0008`  | RNG seed (`u64`); same seed → same corpus. |
| `--facts`      | `10000`          | Minimum fact count to produce. |
| `--knowledge`  | `50000`          | Minimum knowledge-item count to produce. |
| `--klams-url`  | `$KLAMS_URL` or `http://localhost:7777` | klams-service base URL. |
| `--klams-token`| `$KLAMS_TOKEN`   | Bearer token with `write` scope. |
| `--dry-run`    | false            | Skip writes; emit counts the run would produce. |

**Exit codes**:

- `0` — fixture written; counts logged.
- `2` — invalid CLI args or unreachable klams-service.
- `3` — write path failed mid-run (the operator is expected to fix and retry; idempotent via existing dedupe).

**Determinism**: a `klams-bench` integration test
(`tests/unit/klams-bench/fixture_determinism.rs`) instantiates the
corpus generator twice with the same seed and asserts the produced
canonical-hash list is byte-identical.

---

## `run` binary

**Recipe**: `just bench-run`

**Invocation**:

```bash
cargo run --release -p klams-bench --bin run -- \
    --klams-url http://localhost:7777 \
    --klams-token "${KLAMS_TOKEN}" \
    --output sprints/008-activity-observability/perf-baseline.md
```

**Flags**:

| Flag             | Default                                                 | Notes |
|------------------|---------------------------------------------------------|-------|
| `--klams-url`    | `$KLAMS_URL` or `http://localhost:7777`                 | klams-service base URL. |
| `--klams-token`  | `$KLAMS_TOKEN`                                          | Bearer token with `read` scope. |
| `--queries`      | `tools/bench/queries.txt`                               | Query file (one per line). |
| `--repeats`      | `10`                                                    | Calls per query; total samples = `queries × repeats`. |
| `--output`       | `sprints/008-activity-observability/perf-baseline.md`     | Markdown file to write. |
| `--seed-of-store`| (none)                                                  | Optional sanity-check; if set, the harness verifies the store's row counts match what the seed binary would have produced. |

**Behavior**:

1. Loads queries from `--queries`.
2. Calls `klams_client::memory_search` `--repeats` times per query.
3. Records each call's wall-clock latency in `hdrhistogram::Histogram<u64>` (microsecond resolution, range 1 µs – 60 s).
4. Computes p50, p95, p99, min, max, mean.
5. Writes the markdown file at `--output`.
6. **Exits 0 regardless of whether p95 < SC-006's 1 s threshold** (FR-022).

**Exit codes**:

- `0` — markdown written.
- `2` — invalid CLI args, unreachable klams-service, or missing query file.
- `3` — any `memory_search` call returned an error response (the harness aborts to keep the report from claiming success on a broken store).

---

## Query Set Governance

**Location**: `tools/bench/queries.txt`. **Format**: one query per line, UTF-8, blank lines and `#` comment lines ignored. **Size**: exactly 10 queries (matches the `100 samples` headline; if the count changes, update the perf-baseline template).

**Representativeness criteria** (a "representative query" satisfies all four):

1. **Kind coverage** — the set MUST contain at least one query that primarily hits each of the three kinds (`fact`, `knowledge`, `event`). Inferred from the corpus the seed binary produces.
2. **Complexity coverage** — the set MUST contain at least two single-term, at least four multi-term, and at least two combined (multi-term + payload-shape-implied filter) queries.
3. **Payload shape** — queries SHOULD reflect homelab-operator usage (host names like `kubs0`, runbook phrases, incident timestamps, deploy outcomes); avoid synthetic-only strings that never appear in real authoring.
4. **Stability** — every line in `queries.txt` MUST be reproducible from the seeded corpus (`--seed 0xC0FFEE_0008`); the seeded corpus is canonical and the queries are checked in alongside it.

**Update process**: the file is checked in. Edits land via PR with a one-line rationale in the commit message and a re-run of `just bench-run` to refresh `perf-baseline.md` in the same PR. Operator-only changes (no new use case) MUST NOT change the query count; if a query is replaced, the report headline (`100 samples across 10 queries`) stays valid.

---

## `perf-baseline.md` — output format

The `run` binary writes the file from a fixed template. The format is stable so diffs across runs are diff-friendly. Latencies are rendered to **one decimal place in milliseconds**; counts use thousands separators. The metric column is left-aligned and the value column is right-aligned.

**Template** (placeholders in `{…}`; the harness substitutes literal values):

```markdown
# Perf baseline — sprint 008

> Generated {iso8601_utc} by `just bench-run` on `{hostname}`.
> Fixture seed: `{seed_hex}` · Store: {facts:,} facts, {knowledge:,} knowledge items.

## `memory_search` latency ({samples} samples across {queries} queries)

| Metric    |              Value |
| --------- | -----------------: |
| p50       |       {p50:.1} ms |
| p95       |       {p95:.1} ms |
| p99       |       {p99:.1} ms |
| min / max | {min:.1} ms / {max:.1} ms |
| mean      |      {mean:.1} ms |

## Sample queries

{for each query in queries.txt:}
- `"{query}"`

## Notes

- Run against a quiescent store; concurrent writes during the run will skew the numbers.
- SC-006 threshold (`memory_search` p95 < 1 s) is not enforced by this harness; this file surfaces the measurement. Tuning is gated on user review.
```

**Filled example** (illustrative; real output is committed to `sprints/008-activity-observability/perf-baseline.md`):

```markdown
# Perf baseline — sprint 008

> Generated 2026-05-25T17:42:11Z by `just bench-run` on `kubs0`.
> Fixture seed: `0xC0FFEE_0008` · Store: 10,247 facts, 50,138 knowledge items.

## `memory_search` latency (100 samples across 10 queries)

| Metric    |              Value |
| --------- | -----------------: |
| p50       |            18.0 ms |
| p95       |            73.0 ms |
| p99       |           142.0 ms |
| min / max |  11.0 ms / 165.0 ms |
| mean      |            28.0 ms |

## Sample queries

- `"widget deploy outcome"`
- `"kubs0 backup window"`
- ...

## Notes

- Run against a quiescent store; concurrent writes during the run will skew the numbers.
- SC-006 threshold (`memory_search` p95 < 1 s) is not enforced by this harness; this file surfaces the measurement. Tuning is gated on user review.
```

Reruns overwrite the file (idempotent generation; checked-in result
reflects the most recent run).

---

## Tests (drive implementation)

| FR | Test slot |
|----|-----------|
| FR-019 | `tests/unit/klams-bench/fixture_determinism.rs::same_seed_produces_same_corpus` |
| FR-020 | `tools/bench` — integration smoke that runs the harness against the test compose with a tiny query set |
| FR-021 | quickstart §10 — operator opens README, follows the link, sees the file |
| FR-022 | `klams-bench::run::exits_zero_when_p95_exceeds_threshold` — synthetic histogram unit test |
