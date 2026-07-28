# klams-bench

Sprint 008 — non-shipping perf fixture + harness for `memory_search`
(FR-019..FR-022). See
[`sprints/008-activity-observability/contracts/bench-harness.md`](../../sprints/008-activity-observability/contracts/bench-harness.md)
for the authoritative CLI surface and output format, and
[`sprints/008-activity-observability/contracts/bench-harness.md#query-set-governance`](../../sprints/008-activity-observability/contracts/bench-harness.md#query-set-governance)
for the rules governing `queries.txt`.

## Binaries

- `seed` — deterministic seeded fixture generator. Defaults: 10k
  facts + 50k knowledge items, seed `0x0000_C0FF_EE00_0008`.
- `run` — runs the queries in `queries.txt` (10 queries × 10 repeats
  = 100 samples) against `klams-service`, writes
  `sprints/008-activity-observability/perf-baseline.md`.

## Recipes

- `just bench-seed` → `cargo run --release -p klams-bench --bin seed`
- `just bench-run`  → `cargo run --release -p klams-bench --bin run`
- `just bench-clean` → purge every row authored by `klams-bench`
  from Postgres (`facts` / `events` `WHERE author_id = (SELECT id
  FROM authors WHERE agent_name = 'klams-bench')`) and from the
  Qdrant collection (default `knowledge_items_v2`; payload filter
  `author_id = <uuid>`). Needs `PGPASSWORD`; honors `PGHOST`,
  `PGUSER`, `PGDATABASE`, `QDRANT_URL`, `QDRANT_COLLECTION`.

The seed/run recipes always exit 0 (per FR-022) — the harness surfaces
measurement but never gates `just gate`.

Bench rows are identified by author attribution (sprint 009 FR-011):
the bench `$KLAMS_TOKEN` is bound to a dedicated `klams-bench`
agent_name via `klams.toml`, so every fact, event, and knowledge
point it writes carries `author_id = <klams-bench-uuid>`. `just
bench-clean` resolves that UUID and deletes by it directly — no
payload-pattern fallback.

## Environment

| Variable      | Used by    | Notes                                   |
|---------------|------------|-----------------------------------------|
| `KLAMS_URL`   | seed, run  | Defaults to `http://127.0.0.1:7777`.     |
| `KLAMS_TOKEN` | seed, run  | Required. Use a token with `read,write`. |

> **Sprint 009 (FR-007):** the bench `$KLAMS_TOKEN` should be bound
> to a dedicated author. Configure it in `klams.toml` as a scoped
> grant with `agent_name = "klams-bench"` so the rows it writes are
> attributed away from the operational `system` author. See
> [sprints/009-stability-attribution/contracts/token-grant-config.md](../../sprints/009-stability-attribution/contracts/token-grant-config.md).

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
| `--output`       | `sprints/008-activity-observability/perf-baseline.md`   | Markdown output. |
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
