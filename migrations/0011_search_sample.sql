-- Sprint 026 (#643): the search-sample log.
--
-- klams has had no record of what agents actually ask it. The miss log
-- (0010) only fires on a *failure* classification, and its threshold was
-- mis-calibrated badly enough that it recorded one row in two weeks — so
-- in practice there was no query record at all. That left two things
-- impossible:
--
--   1. Mining real agent phrasing for the retrieval eval suite. Every
--      eval query to date was invented by whoever wrote the suite.
--   2. Observing the score distribution, which is the only honest way to
--      set the miss-log threshold. Sprint 026 calibrated it from #628's
--      handful of data points; the next calibration (after the #655 GPU
--      model swap changes the distribution wholesale) should come from
--      this table instead.
--
-- Records every search, not a subset — klams serves agents, not public
-- traffic, so the volume is low enough that sampling would cost more
-- information than it saves. The name keeps the WI's terminology; if
-- volume ever justifies a rate, this is where it lands. Written
-- fire-and-forget off the search path exactly like search_miss, so an
-- insert failure never affects a live search. Append-only; retention is
-- an operator concern (periodic prune by created_at), not enforced here.

CREATE TABLE IF NOT EXISTS search_sample (
    id             BIGSERIAL   PRIMARY KEY,
    query          TEXT        NOT NULL,
    caller         TEXT        NOT NULL,
    -- Pre-fusion relevance of the top hit (Qdrant cosine for knowledge,
    -- Postgres ts_rank for fact/event). NULL when nothing came back.
    -- This column is the score distribution the threshold calibrates on,
    -- so it is deliberately the RAW score, never the fused RRF value.
    top_raw_score  REAL        NULL,
    -- Kind of that top hit, because the raw scales are not comparable
    -- across kinds — a distribution over mixed kinds is meaningless.
    top_kind       TEXT        NULL,   -- 'knowledge' | 'fact' | 'event'
    hit_count      INTEGER     NOT NULL,
    kinds          TEXT        NOT NULL,   -- comma-joined kinds queried
    -- Sprint 026 (#641) landed query-time duplicate collapse; this is
    -- its live effect, per search. Makes the "half of every result page
    -- was wasted" claim measurable in production rather than only in the
    -- one-off corpus scan that motivated it.
    duplicates_collapsed INTEGER NOT NULL DEFAULT 0,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS search_sample_created_at_idx
    ON search_sample (created_at DESC);

-- The calibration query ("what does the score distribution look like for
-- knowledge-topped searches") filters on kind and orders by score.
CREATE INDEX IF NOT EXISTS search_sample_top_kind_score_idx
    ON search_sample (top_kind, top_raw_score);
