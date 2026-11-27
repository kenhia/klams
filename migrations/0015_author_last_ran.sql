-- Sprint 061 (WI 4639): each scanner tells klams it ran — a heartbeat
-- per run, `POST /v1/authors/me/ran` — and `/v1/authors` exposes it per
-- author. Kept apart from `last_seen_at` on purpose: that is a WRITE
-- stamp, and a scanner that runs and finds nothing new still ran.
ALTER TABLE authors ADD COLUMN IF NOT EXISTS last_ran_at TIMESTAMPTZ NULL;
ALTER TABLE authors ADD COLUMN IF NOT EXISTS last_run_ok BOOLEAN NULL;
