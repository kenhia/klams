# Contract — `reattribute-system` CLI

## Invocation

```bash
reattribute-system --dry-run [--report-out path]
reattribute-system --apply   [--report-out path]
```

Exactly one of `--dry-run` / `--apply` is required. When neither is
present, exit code 2 with a clarifying error message.

## Connection

Reads `KLAMS_DATABASE_URL` and `KLAMS_QDRANT_URL` from environment.
Falls back to the same defaults as `klams-service`. No bearer token
needed — this is an offline admin tool that connects directly to the
stores.

## Behavior

For each of `facts`, `events`, `knowledge_items`:

1. Select all rows where `author_id = SYSTEM_AUTHOR_ID`.
2. For each row, run the provenance lookup (see
   [research.md](../research.md) R4).
3. Classify into:
   - `reassigned_to_recovered_author` (single non-system author
     implicated, and that author still exists in the store)
   - `reassigned_to_lost_author` (no non-system events found, OR
     multiple non-system authors implicated, OR the recovered
     author no longer exists in `authors`)
   - `left_as_system` (row's original write was genuinely the
     seeded `system` identity — detected by the absence of any
     `system`-distinguishing provenance signal)
4. In `--apply` mode, write the new `author_id` (Postgres `UPDATE`
   for facts/events, Qdrant payload update for knowledge_items).
   The `lost-author` bucket is written as
   `author_id = LOST_AUTHOR_ID`; the `system` bucket is not
   touched. In `--dry-run` mode, count without writing.

Chunking: batch updates of 500 rows per transaction to avoid long
table locks.

## Report shape (stdout + optional file)

```json
{
  "started_at": "2026-05-27T18:00:00Z",
  "completed_at": "2026-05-27T18:00:04Z",
  "mode": "dry_run",
  "facts": {
    "total_system_attributed": 22,
    "reassigned_to_recovered_author": 18,
    "reassigned_to_lost_author": 3,
    "left_as_system": 1,
    "per_author": [
      {"author_id": "...", "agent_name": "alice",       "count": 12},
      {"author_id": "...", "agent_name": "bob",         "count":  6},
      {"author_id": "...", "agent_name": "lost-author", "count":  3}
    ]
  },
  "events": { "...": "..." },
  "knowledge_items": { "...": "..." }
}
```

**Invariant**: for every table,
`total_system_attributed == reassigned_to_recovered_author +
reassigned_to_lost_author + left_as_system`, and the table's overall
row count is identical before and after `--apply` (FR-016).

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Completed successfully (with or without reassignments) |
| 1 | Store error (DB or Qdrant unreachable; transaction failure) |
| 2 | Usage error (missing/conflicting flags) |
| 3 | Config error (env vars missing) |

## Idempotency

Running `--apply` twice must produce identical reports on the second
run with `reassigned_to_recovered_author = 0` and
`reassigned_to_lost_author = 0` for every table.

## Contract tests

Located in `crates/klams-store/tests/repair.rs`:

- T1: seed 3 facts attributed to `system` with provenance pointing
  to alice; dry-run reports `reassigned_to_recovered_author = 3`;
  apply reassigns; second apply reports all reassignment counters
  at 0.
- T2: seed 1 fact with conflicting provenance (alice + bob); apply
  reassigns it to `lost-author` and counts it under
  `reassigned_to_lost_author`.
- T3: seed 1 fact with no provenance event; reassigned to
  `lost-author` and counted under `reassigned_to_lost_author`.
- T4: knowledge_items payload gets `author_id` and
  `author_agent_name` after `--apply` (including the `lost-author`
  case).
- T5: seed 1 fact whose provenance points to an author row that
  has since been deleted from `authors`; reassigned to
  `lost-author` and counted under `reassigned_to_lost_author`.
- T6: total row count of each table is identical before and after
  `--apply` (FR-016 invariant).
