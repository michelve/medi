-- V3__libraries.sql — Plex-style named libraries + their folders (Phase B).
--
-- Canonical source: docs/.tasks/60-metadata-and-libraries.md §DB migrations (Phase B).
-- Introduces user-managed libraries: each library has a kind ('movie' | 'series') and
-- one or more folders (each an absolute path that must resolve under MEDIA_DIR — the
-- containment check lives in the API layer, not the DDL). Existing catalog rows are
-- scoped to a library so scans and reaps become per-library.
--
-- Auto-seed: on first boot with no libraries defined, the api crate seeds one 'movie'
-- and one 'series' library rooted at MEDIA_DIR and back-fills library_id on existing
-- rows, so a single-/media deployment keeps working with no config change. That seeding
-- is data, not schema, so it is done in code (medi_db::writes::seed_default_libraries)
-- rather than here — this migration is pure DDL and idempotent via refinery.

CREATE TABLE libraries (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,                     -- 'movie' | 'series'
    created_at INTEGER NOT NULL
);

CREATE TABLE library_folders (
    id         INTEGER PRIMARY KEY,
    library_id INTEGER NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,                     -- absolute, must resolve under MEDIA_DIR
    UNIQUE(library_id, path)
);

ALTER TABLE movies ADD COLUMN library_id INTEGER REFERENCES libraries(id) ON DELETE CASCADE;
ALTER TABLE series ADD COLUMN library_id INTEGER REFERENCES libraries(id) ON DELETE CASCADE;

CREATE INDEX idx_movies_library ON movies(library_id);
CREATE INDEX idx_series_library ON series(library_id);
CREATE INDEX idx_library_folders_lib ON library_folders(library_id);
