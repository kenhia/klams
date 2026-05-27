# klams-bench

Sprint 008 — non-shipping perf fixture + harness for `memory_search`
(FR-019..FR-022). See
[`specs/008-activity-observability/contracts/bench-harness.md`](../../specs/008-activity-observability/contracts/bench-harness.md)
for the authoritative CLI surface and output format, and
[`specs/008-activity-observability/contracts/bench-harness.md#query-set-governance`](../../specs/008-activity-observability/contracts/bench-harness.md#query-set-governance)
for the rules governing `queries.txt`.

## Binaries

- `seed` — deterministic seeded fixture generator. Defaults: 10k
  facts + 50k knowledge items, seed `0x0000_C0FF_EE00_0008`.
- `run` — runs the queries in `queries.txt` (10 queries × 10 repeats
  = 100 samples) against `klams-service`, writes
  `specs/008-activity-observability/perf-baseline.md`.

## Recipes

- `just bench-seed` → `cargo run --release -p klams-bench --bin seed`
- `just bench-run`  → `cargo run --release -p klams-bench --bin run`
- `just bench-clean` → purge bench-seeded facts (Postgres) and bench
  knowledge points (Qdrant). Needs `PGPASSWORD`; honors `PGHOST`,
  `PGUSER`, `PGDATABASE`, `QDRANT_URL`, `QDRANT_COLLECTION`.

The seed/run recipes always exit 0 (per FR-022) — the harness surfaces
measurement but never gates `just gate`.

Bench rows are identified by their generator payload markers
(`tools/bench/src/lib.rs`): `UserFact.name = "bench-user-*"`,
`EnvFact.key = "BENCH_*"`, `TaskFact.task_id = "ansible-*"` with a `seq`
field, and Qdrant points where `repo = "klams"` and `file` starts with
`notes/`. See backlog item "Per-token author attribution" for the
durable fix that would let cleanup target a dedicated author instead.

## Environment

| Variable      | Used by    | Notes                                   |
|---------------|------------|-----------------------------------------|
| `KLAMS_URL`   | seed, run  | Defaults to `http://127.0.0.1:7777`.     |
| `KLAMS_TOKEN` | seed, run  | Required. Use a token with `read,write`. |

## seed flags

| Flag             | Default                | Notes                                       |
|------------------|------------------------|---------------------------------------------|
| `--seed`         | `0x0000_C0FF_EE00_0008` | Hex or decimal; deterministic.              |
| `--facts`        | `10000`                 | Fact count to write.                        |
| `--knowledge`    | `50000`                 | Knowledge-item count to write.              |
| `--klams-url`    | `$KLAMS_URL`            | Service base URL.                           |
| `--klams-token`  | `$KLAMS_TOKEN`          | Bearer token.                               |
| `--dry-run`      | false                   | Generate corpus, skip writes.               |

The seed binary retries 503 `queue_full` responses with exponential
backoff so a slow embedder doesn't drop rows.

## run flags

| Flag             | Default                                              | Notes |
|------------------|------------------------------------------------------|-------|
| `--klams-url`    | `$KLAMS_URL`                                         | Service base URL. |
| `--klams-token`  | `$KLAMS_TOKEN`                                       | Bearer token. |
| `--queries`      | `tools/bench/queries.txt`                             | Query file. |
| `--repeats`      | `10`                                                  | Calls per query. |
| `--output`       | `specs/008-activity-observability/perf-baseline.md`   | Markdown output. |
| `--facts`        | `10000`                                               | Header metadata only. |
| `--knowledge`    | `50000`                                               | Header metadata only. |
| `--seed`         | `0x0000_C0FF_EE00_0008`                                | Header metadata only. |

When `--facts` or `--knowledge` is below the canonical defaults the
generated markdown is auto-tagged as a smoke run.

## Determinism

`tests/fixture_determinism.rs` asserts that the same seed yields a
byte-identical corpus and that distinct seeds diverge. This is the
contract behind FR-019.

## Smoke vs full baseline

The committed `perf-baseline.md` is currently a **smoke-sized** run
(500 facts / 2,000 knowledge items) to avoid bloating the live store
while kwi work item #26 (loopback
CLOSE_WAIT leak) is open. Rerun with the full corpus
(`10000 / 50000`) once that bug is fixed; the markdown header will
drop its "Smoke run" footer automatically.
