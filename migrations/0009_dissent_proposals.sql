-- Sprint 015: external dissent proposals (MCP `dissent_propose`).
--
-- Three nullable provenance columns for dissents filed directly by an
-- agent (semantic contradiction detection) rather than diverted on the
-- write path. Write-path dissents leave all three NULL — their
-- provenance is the `source` trust tier, unchanged.

ALTER TABLE dissents
    ADD COLUMN IF NOT EXISTS reason TEXT NULL,
    ADD COLUMN IF NOT EXISTS contradicting_memory_id UUID NULL,
    ADD COLUMN IF NOT EXISTS author_id UUID NULL REFERENCES authors (id);

CREATE INDEX IF NOT EXISTS dissents_author_id_idx
    ON dissents (author_id) WHERE author_id IS NOT NULL;
