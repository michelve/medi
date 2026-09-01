-- V2__metadata.sql — external ids + match state for metadata enrichment.
--
-- Canonical source: docs/.tasks/60-metadata-and-libraries.md §DB migrations (Phase A).
-- The descriptive columns enrichment fills (overview, poster_path, backdrop_path, and
-- the people/credits tables) already exist from V1; this migration adds only the
-- provider linkage (tmdb_id / imdb_id) and the per-title match lifecycle used to keep
-- enrichment idempotent (a 'matched' row is never re-fetched unless force-refreshed).
--
-- metadata_state values: 'pending' | 'matched' | 'unmatched' | 'failed'
--   pending   — ingested, not yet enriched
--   matched   — a provider match was found and its details written
--   unmatched — no candidate cleared the match threshold (do not retry automatically)
--   failed    — the provider call errored (transient; a refresh may retry)

ALTER TABLE movies ADD COLUMN tmdb_id        INTEGER;
ALTER TABLE movies ADD COLUMN imdb_id        TEXT;
ALTER TABLE movies ADD COLUMN metadata_state TEXT NOT NULL DEFAULT 'pending';

ALTER TABLE series ADD COLUMN tmdb_id        INTEGER;
ALTER TABLE series ADD COLUMN imdb_id        TEXT;
ALTER TABLE series ADD COLUMN metadata_state TEXT NOT NULL DEFAULT 'pending';

-- The enrichment worker selects 'pending'/'failed' rows to process; the index keeps
-- that scan cheap even on a 10k-title library.
CREATE INDEX idx_movies_meta_state ON movies(metadata_state);
CREATE INDEX idx_series_meta_state ON series(metadata_state);
