-- Sprint 007: MCP memory server — author registry.
-- Additive: introduces the `authors` table and seeds the SYSTEM_AUTHOR_ID row.
-- All pre-MCP rows in facts/events are backfilled to reference this author
-- in migrations 0006 and 0007.

CREATE TABLE IF NOT EXISTS authors (
    id              UUID PRIMARY KEY,
    agent_name      TEXT NOT NULL,
    model           TEXT,
    session_title   TEXT,
    repo            TEXT,
    client_app      TEXT,
    client_version  TEXT,
    extra           JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_authors_agent_name   ON authors (agent_name);
CREATE INDEX IF NOT EXISTS idx_authors_last_seen_at ON authors (last_seen_at DESC);

-- System author — referenced by SYSTEM_AUTHOR_ID in klams-types.
INSERT INTO authors (id, agent_name, model, client_app, created_at, last_seen_at)
VALUES (
    '00000000-0000-7000-8000-000000000001'::uuid,
    'system',
    NULL,
    'klams-service',
    now(),
    now()
)
ON CONFLICT (id) DO NOTHING;
