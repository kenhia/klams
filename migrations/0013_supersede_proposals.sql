-- Sprint 058 (klams-mind WI 3331): supersede proposals.
--
-- An agent without the authority to supersede another author's
-- knowledge files a proposal here (MCP `supersede_propose`); a `manage`
-- identity accepts or rejects it (`supersede_proposal_resolve`), and
-- acceptance performs the supersede in Qdrant. Knowledge lives in
-- Qdrant, so the ids below are point ids and carry no foreign key.

CREATE TABLE IF NOT EXISTS supersede_proposals (
    id                     UUID PRIMARY KEY,
    author_id              UUID NOT NULL REFERENCES authors (id),
    keeper_id              UUID NOT NULL,
    retire_ids             UUID[] NOT NULL,
    text                   TEXT NULL,
    reason                 TEXT NOT NULL,
    proposal_hash          TEXT NOT NULL,
    status                 TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'accepted', 'rejected', 'stale')),
    submission_count       INTEGER NOT NULL DEFAULT 1,
    submitted_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at            TIMESTAMPTZ NULL,
    resolved_by_author_id  UUID NULL REFERENCES authors (id),
    result_memory_id       UUID NULL,
    resolution_note        TEXT NULL,
    CHECK (cardinality(retire_ids) >= 1)
);

-- A re-filed identical proposal bumps the pending row rather than
-- queueing a second copy (klams-mind re-runs consolidation).
CREATE UNIQUE INDEX IF NOT EXISTS supersede_proposals_pending_dedupe
    ON supersede_proposals (proposal_hash) WHERE status = 'pending';

CREATE INDEX IF NOT EXISTS supersede_proposals_status_idx
    ON supersede_proposals (status, submitted_at);
