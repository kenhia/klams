-- Sprint 027 (#656): the oversize-write log.
--
-- Records knowledge writes refused because the text exceeds the
-- embedding model's input ceiling. Agents hitting that ceiling improvise
-- ("Payload too large — splitting into focused records") and until now
-- klams kept no record that it happened: the coherent original — the
-- thing the agent actually wanted to store — was simply lost, with no
-- data on how often, by how much, or by whom.
--
-- `text` holds the FULL submitted payload, deliberately. It was content
-- destined for the store anyway, and it is the "what did we lose" corpus:
-- the only way to answer whether a hand-split preserved the content or
-- dropped its tail. That also makes this table the one place klams
-- retains rejected user content, hence the retention cap below.
--
-- Written fire-and-forget off the write path (same discipline as
-- `search_miss`, migration 0010), so a logging failure never changes what
-- the caller sees.
--
-- After the sprint-028 model upgrade (512 -> 8k+ tokens) this becomes a
-- rare-event log, which is exactly when individual rows are worth reading
-- one at a time — and it is the instrument that decides whether #632's
-- server-side chunking is ever actually needed.

CREATE TABLE IF NOT EXISTS oversize_write (
    id               BIGSERIAL   PRIMARY KEY,
    author_id        UUID        NULL REFERENCES authors (id) ON DELETE SET NULL,
    agent_name       TEXT        NOT NULL,
    submitted_chars  INTEGER     NOT NULL,
    estimated_tokens INTEGER     NOT NULL,
    limit_tokens     INTEGER     NOT NULL,   -- the ceiling in force at the time
    max_chars        INTEGER     NOT NULL,   -- what the caller was told to split below
    text             TEXT        NOT NULL,   -- the full rejected payload
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS oversize_write_created_at_idx
    ON oversize_write (created_at DESC);

-- Answers "which agents keep hitting this, and how hard".
CREATE INDEX IF NOT EXISTS oversize_write_agent_idx
    ON oversize_write (agent_name, created_at DESC);
