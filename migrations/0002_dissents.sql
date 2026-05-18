-- Sprint 002: dissents table + facts.dissent_count + triggers.
-- See specs/002-safety-and-write-ops/data-model.md.

-- ---------- facts.dissent_count ----------
ALTER TABLE facts
    ADD COLUMN IF NOT EXISTS dissent_count INT NOT NULL DEFAULT 0;

-- ---------- dissents ----------
CREATE TABLE IF NOT EXISTS dissents (
    id                  UUID PRIMARY KEY,
    fact_id             UUID NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    proposed_payload    JSONB NOT NULL,
    payload_hash        BYTEA NOT NULL,
    source              TEXT NOT NULL,
    submitted_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    submission_count    INT NOT NULL DEFAULT 1,
    status              TEXT NOT NULL DEFAULT 'pending',
    resolved_at         TIMESTAMPTZ NULL,
    resolved_by_source  TEXT NULL,
    CONSTRAINT dissents_status_check
        CHECK (status IN ('pending', 'promoted', 'discarded', 'orphaned'))
);

CREATE INDEX IF NOT EXISTS dissents_fact_id_idx     ON dissents (fact_id);
CREATE INDEX IF NOT EXISTS dissents_status_idx      ON dissents (status);
CREATE INDEX IF NOT EXISTS dissents_pending_age_idx ON dissents (submitted_at)
    WHERE status = 'pending';

-- FR-013 dedupe: at most one pending dissent per (fact, payload).
CREATE UNIQUE INDEX IF NOT EXISTS dissents_pending_dedupe_idx
    ON dissents (fact_id, payload_hash) WHERE status = 'pending';

-- ---------- triggers ----------
CREATE OR REPLACE FUNCTION refresh_fact_dissent_count(p_fact_id UUID)
RETURNS VOID AS $$
BEGIN
    UPDATE facts
       SET dissent_count = (
           SELECT count(*) FROM dissents
            WHERE fact_id = p_fact_id AND status = 'pending'
       )
     WHERE id = p_fact_id;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION refresh_fact_dissent_count_tg()
RETURNS trigger AS $$
BEGIN
    -- For INSERT NEW.fact_id is the target; for UPDATE we only fire
    -- on status changes (per the trigger predicate below) and the
    -- target is still NEW.fact_id (fact_id is immutable on dissents).
    PERFORM refresh_fact_dissent_count(NEW.fact_id);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION orphan_pending_dissents_tg()
RETURNS trigger AS $$
BEGIN
    -- Mark every still-pending dissent for the fact about to be
    -- deleted as 'orphaned' so the resolution is deterministic and
    -- observable before ON DELETE CASCADE physically removes the
    -- rows. Consumers see the transition via the metrics counter
    -- klams_dissents_total{outcome="orphaned"}.
    UPDATE dissents
       SET status = 'orphaned',
           resolved_at = now(),
           resolved_by_source = NULL
     WHERE fact_id = OLD.id AND status = 'pending';
    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS dissents_after_insert ON dissents;
CREATE TRIGGER dissents_after_insert
    AFTER INSERT ON dissents
    FOR EACH ROW EXECUTE FUNCTION refresh_fact_dissent_count_tg();

DROP TRIGGER IF EXISTS dissents_after_status_update ON dissents;
CREATE TRIGGER dissents_after_status_update
    AFTER UPDATE OF status ON dissents
    FOR EACH ROW
    WHEN (OLD.status IS DISTINCT FROM NEW.status)
    EXECUTE FUNCTION refresh_fact_dissent_count_tg();

DROP TRIGGER IF EXISTS facts_before_delete_orphan_dissents ON facts;
CREATE TRIGGER facts_before_delete_orphan_dissents
    BEFORE DELETE ON facts
    FOR EACH ROW EXECUTE FUNCTION orphan_pending_dissents_tg();
