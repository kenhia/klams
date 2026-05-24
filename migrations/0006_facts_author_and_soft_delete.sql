-- Sprint 007: MCP memory server — facts attribution + soft delete.
-- Additive: new columns + FK constraints + indexes. Existing rows are
-- backfilled to SYSTEM_AUTHOR_ID before NOT NULL is enforced.

ALTER TABLE facts
    ADD COLUMN IF NOT EXISTS author_id            UUID,
    ADD COLUMN IF NOT EXISTS deleted_at           TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS deleted_by_author_id UUID;

UPDATE facts
   SET author_id = '00000000-0000-7000-8000-000000000001'::uuid
 WHERE author_id IS NULL;

ALTER TABLE facts
    ALTER COLUMN author_id SET NOT NULL;

ALTER TABLE facts
    ADD CONSTRAINT facts_author_id_fkey
        FOREIGN KEY (author_id) REFERENCES authors(id),
    ADD CONSTRAINT facts_deleted_by_fkey
        FOREIGN KEY (deleted_by_author_id) REFERENCES authors(id);

CREATE INDEX IF NOT EXISTS idx_facts_author_id  ON facts (author_id);
CREATE INDEX IF NOT EXISTS idx_facts_deleted_at ON facts (deleted_at)
    WHERE deleted_at IS NOT NULL;
