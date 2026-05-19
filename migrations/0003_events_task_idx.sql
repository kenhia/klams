-- no-transaction
--
-- Sprint 003 (FR-010): expression index on events.payload->>'task_id'
-- to make per-task trace queries sub-linear.
--
-- The `-- no-transaction` directive on line 1 is recognised by
-- sqlx 0.8 and tells the migrator to run this file without
-- wrapping it in BEGIN/COMMIT, which is required for
-- CREATE INDEX CONCURRENTLY (Postgres rejects it inside a tx block).
--
-- IF NOT EXISTS guards against re-runs after an interrupted
-- CONCURRENTLY build (which would otherwise leave an invalid index).

CREATE INDEX CONCURRENTLY IF NOT EXISTS events_task_id_created_at_idx
    ON events ((payload->>'task_id'), created_at)
    WHERE category IN ('Execution', 'Service');
