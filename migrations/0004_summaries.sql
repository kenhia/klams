-- Sprint 005 (Phase 4) — summaries table.
-- See specs/005-advanced-retrieval/data-model.md §3.

CREATE TABLE IF NOT EXISTS summaries (
    id              UUID PRIMARY KEY,
    kind            TEXT NOT NULL CHECK (kind IN ('event')),
    host            TEXT NOT NULL,
    category        TEXT NOT NULL,
    day_bucket      DATE NOT NULL,
    source_count    INTEGER NOT NULL,
    source_ids      UUID[] NOT NULL,
    summary_text    TEXT NOT NULL,
    mechanism       TEXT NOT NULL CHECK (mechanism IN ('extractive', 'llm')),
    generated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    invalidated_at  TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS summaries_unique_cluster_idx
    ON summaries (kind, host, category, day_bucket);

CREATE INDEX IF NOT EXISTS summaries_day_bucket_idx
    ON summaries (day_bucket DESC);

CREATE INDEX IF NOT EXISTS summaries_category_idx
    ON summaries (category);

CREATE INDEX IF NOT EXISTS summaries_invalidated_idx
    ON summaries (invalidated_at)
    WHERE invalidated_at IS NULL;
