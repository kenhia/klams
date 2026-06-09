-- Sprint 009 (FR-016a): seed the `lost-author` identity so the
-- re-attribution repair has a stable, queryable destination for
-- rows whose true writer cannot be unambiguously recovered.
-- Backfill into existing tables happens only when the operator
-- runs `reattribute-system --apply`; this migration is purely
-- additive and idempotent.

INSERT INTO authors (id, agent_name, model, client_app, created_at, last_seen_at)
VALUES (
    '00000000-0000-7000-8000-000000000002'::uuid,
    'lost-author',
    NULL,
    'klams-service',
    now(),
    now()
)
ON CONFLICT (id) DO NOTHING;
