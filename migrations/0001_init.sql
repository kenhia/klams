-- Initial schema for klams MVP: facts + events.
-- All tables ship with the indexes called out in
-- sprints/001-initial-mvp/data-model.md.

CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ---------- facts ----------
CREATE TABLE IF NOT EXISTS facts (
    id            UUID PRIMARY KEY,
    type          TEXT NOT NULL,
    payload       JSONB NOT NULL,
    payload_hash  BYTEA NOT NULL,
    version       INT NOT NULL DEFAULT 1,
    source        TEXT NOT NULL,
    confidence    REAL NOT NULL DEFAULT 1.0,
    decay_weight  REAL NOT NULL DEFAULT 1.0,
    use_count     INT NOT NULL DEFAULT 0,
    last_used_at  TIMESTAMPTZ NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    tsv           TSVECTOR GENERATED ALWAYS AS (to_tsvector('english', payload::text)) STORED
);

CREATE INDEX IF NOT EXISTS facts_type_idx       ON facts (type);
CREATE INDEX IF NOT EXISTS facts_source_idx     ON facts (source);
CREATE INDEX IF NOT EXISTS facts_created_at_idx ON facts (created_at);
CREATE INDEX IF NOT EXISTS facts_payload_gin    ON facts USING GIN (payload jsonb_path_ops);
CREATE INDEX IF NOT EXISTS facts_tsv_gin        ON facts USING GIN (tsv);
CREATE UNIQUE INDEX IF NOT EXISTS facts_type_payload_hash_idx
    ON facts (type, payload_hash);

-- ---------- events ----------
CREATE TABLE IF NOT EXISTS events (
    id          UUID PRIMARY KEY,
    task_id     UUID NULL,
    category    TEXT NOT NULL,
    payload     JSONB NOT NULL,
    source      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    tsv         TSVECTOR GENERATED ALWAYS AS (to_tsvector('english', payload::text)) STORED
);

CREATE INDEX IF NOT EXISTS events_task_id_idx
    ON events (task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS events_category_idx   ON events (category);
CREATE INDEX IF NOT EXISTS events_created_at_idx ON events (created_at);
CREATE INDEX IF NOT EXISTS events_tsv_gin        ON events USING GIN (tsv);
