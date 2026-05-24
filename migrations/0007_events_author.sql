-- Sprint 007: MCP memory server — events attribution.
-- Additive: events are append-only, so no soft-delete columns. Author
-- attribution mirrors facts: backfill SYSTEM_AUTHOR_ID, then NOT NULL + FK.

ALTER TABLE events
    ADD COLUMN IF NOT EXISTS author_id UUID;

UPDATE events
   SET author_id = '00000000-0000-7000-8000-000000000001'::uuid
 WHERE author_id IS NULL;

ALTER TABLE events
    ALTER COLUMN author_id SET NOT NULL;

ALTER TABLE events
    ADD CONSTRAINT events_author_id_fkey
        FOREIGN KEY (author_id) REFERENCES authors(id);

CREATE INDEX IF NOT EXISTS idx_events_author_id ON events (author_id);
