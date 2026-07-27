-- Sprint 021 (#317): the miss log.
--
-- Records `memory_search` results that returned nothing (zero_hit) or
-- only a weak top match (low_score) — the "what did an agent want and
-- not get" feedback loop that drives chunking fixes (022), new scan
-- sources, and the lexical-search decision (024). Written
-- fire-and-forget off the MCP search path, so an insert failure never
-- affects a live search. Append-only; retention is an operator concern
-- (periodic prune by created_at), not enforced here.

CREATE TABLE IF NOT EXISTS search_miss (
    id         BIGSERIAL   PRIMARY KEY,
    query      TEXT        NOT NULL,
    caller     TEXT        NOT NULL,
    reason     TEXT        NOT NULL,   -- 'zero_hit' | 'low_score'
    top_score  REAL        NULL,       -- NULL when zero hits
    hit_count  INTEGER     NOT NULL,
    kinds      TEXT        NOT NULL,   -- comma-joined kinds queried
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS search_miss_created_at_idx
    ON search_miss (created_at DESC);
