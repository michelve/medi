-- V7__collections_trailers.sql — TMDB collections (franchises) + trailer videos (Task 91
-- detail-page extensions).
--
-- The next free refinery version after V6__genres_people.sql. Additive DDL only (no PRAGMAs
-- — see migrations/README.md); idempotent via refinery version records.
--
-- Two capabilities land here, both parsed from the SAME `/movie/{id}` details response the
-- enrichment pipeline already fetches (videos via append_to_response, belongs_to_collection
-- inline) — no extra TMDB request per title:
--   1. Collections — TMDB `belongs_to_collection` groups a franchise's films ("Pirates of
--      the Caribbean Collection"). A canonical `collections` table keyed by TMDB's own
--      collection id, plus a nullable `movies.collection_id` FK, so the detail page can show
--      the OTHER in-library films in the same franchise.
--   2. Trailers — TMDB `videos` (YouTube keys). One row per trailer/teaser of a movie so the
--      detail page can offer a trailer. Only YouTube-hosted video keys are stored (the only
--      site the client embeds); a re-match replaces a movie's trailer set wholesale.

-- Canonical collections, keyed by TMDB collection id (NOT autoincrement) so a re-match
-- upserts the same row. `poster_path` is the collection's own art (relative to images_dir()),
-- downloaded like a title poster.
CREATE TABLE collections (
    id          INTEGER PRIMARY KEY,       -- TMDB collection id (NOT autoincrement)
    name        TEXT NOT NULL,
    poster_path TEXT                        -- relative to images_dir(): collections/<id>/poster.jpg
);

-- A movie belongs to at most one collection. Nullable (most movies have none); ON DELETE
-- SET NULL so removing a collection row never cascades away its movies.
ALTER TABLE movies ADD COLUMN collection_id INTEGER REFERENCES collections(id) ON DELETE SET NULL;

-- Trailers: 0..N per movie. `youtube_key` is the YouTube video id; `kind` is TMDB's `type`
-- ("Trailer" | "Teaser" | "Clip" | …); `ord` preserves the provider's ordering so the
-- "official trailer" (first) surfaces first. A re-match delete-then-inserts the whole set.
CREATE TABLE trailers (
    id          INTEGER PRIMARY KEY,
    movie_id    INTEGER NOT NULL REFERENCES movies(id) ON DELETE CASCADE,
    youtube_key TEXT NOT NULL,
    name        TEXT,
    kind        TEXT,                        -- 'Trailer' | 'Teaser' | 'Clip' | ...
    ord         INTEGER NOT NULL DEFAULT 0,
    UNIQUE(movie_id, youtube_key)
);

-- Fast "the other movies in this collection" and "this movie's trailers".
CREATE INDEX idx_movies_collection ON movies(collection_id);
CREATE INDEX idx_trailers_movie    ON trailers(movie_id);
