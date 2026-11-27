-- Sprint 061 (WI 4835): a supersede proposal may retire nothing when it
-- carries replacement text — a single-record correction ("replace the
-- keeper with this text"). A proposal with neither is still refused.
ALTER TABLE supersede_proposals
    DROP CONSTRAINT IF EXISTS supersede_proposals_retire_ids_check;
ALTER TABLE supersede_proposals
    DROP CONSTRAINT IF EXISTS supersede_proposals_retire_or_text_check;
ALTER TABLE supersede_proposals
    ADD CONSTRAINT supersede_proposals_retire_or_text_check
    CHECK (cardinality(retire_ids) >= 1 OR text IS NOT NULL);
