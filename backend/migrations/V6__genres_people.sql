-- V6__genres_people.sql — TMDB genre categorization + person-enrichment columns (Task 91).
--
-- Canonical source: docs/.tasks/91-genres-and-people-discovery.md §DB migrations.
-- The next free refinery version after V5__subtitle_streams.sql. Additive DDL only
-- (no PRAGMAs — see migrations/README.md); idempotent via refinery version records.
--
-- Two capabilities land here:
--   1. Genres — a canonical `genres` table keyed by TMDB's own genre ids (stable across
--      the API, so re-enrichment is a stable upsert and two providers could map onto the
--      same rows), plus separate movie/series M:N join tables that cascade cleanly on a
--      title delete (reap / library-delete).
--   2. Person enrichment — TMDB linkage + headshot/bio columns on the existing `people`
--      table (whose only column today is `name`). Phase B (person pages) fills these; the
--      columns land now so the genre migration and the person migration are one version.

-- Canonical genres, keyed by TMDB genre id ("Science Fiction" = 878). `id` is NOT
-- autoincrement — it is the provider's id, so a re-match upserts the same row.
CREATE TABLE genres (
    id   INTEGER PRIMARY KEY,          -- TMDB genre id (NOT autoincrement)
    name TEXT NOT NULL UNIQUE
);

-- M:N: a title has many genres, a genre has many titles. Separate movie/series joins keep
-- the FKs simple and cascade on title delete (a reap or library-delete removes the joins).
CREATE TABLE movie_genres (
    movie_id INTEGER NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
    genre_id INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (movie_id, genre_id)
);
CREATE TABLE series_genres (
    series_id INTEGER NOT NULL REFERENCES series(id) ON DELETE CASCADE,
    genre_id  INTEGER NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
    PRIMARY KEY (series_id, genre_id)
);

-- Person enrichment. `people` already exists (id, name UNIQUE); add TMDB linkage + art/bio.
-- `photo_path` is keyed by our internal `people.id` (people/<people.id>/photo.jpg) so a
-- person with no TMDB match still has a stable art path; `tmdb_id` is the link-out, unique
-- when present. Filled by Phase B enrichment/backfill; nullable until then.
ALTER TABLE people ADD COLUMN tmdb_id     INTEGER;   -- TMDB person id (nullable: pre-backfill)
ALTER TABLE people ADD COLUMN photo_path  TEXT;      -- relative to images_dir(): people/<id>/photo.jpg
ALTER TABLE people ADD COLUMN biography   TEXT;

-- Fast "titles in this genre, newest first" and "genres with a nonzero count".
CREATE INDEX idx_movie_genres_genre  ON movie_genres(genre_id);
CREATE INDEX idx_series_genres_genre ON series_genres(genre_id);
-- One row per TMDB person id (a partial unique index so multiple NULLs — pre-backfill
-- people — coexist, while a matched person links to exactly one TMDB id).
CREATE UNIQUE INDEX idx_people_tmdb  ON people(tmdb_id) WHERE tmdb_id IS NOT NULL;
